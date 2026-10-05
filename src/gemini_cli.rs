//! Configure Google's Gemini CLI independently of the Gemini Codex profile.
use crate::config::{Config, Mode};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{fs, path::Path};

fn native_name(name: &str) -> Option<&str> {
    let name = name.strip_prefix("gemini/").unwrap_or(name);
    let name = name.strip_prefix("models/").unwrap_or(name);
    (!name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)))
    .then_some(name)
}

fn model<'a>(config: &'a Config, requested: Option<&'a str>) -> Result<&'a str> {
    if config.mode != Mode::Client && config.gemini.is_none() {
        bail!("Configure providers.gemini before configuring Gemini CLI");
    }
    let usable = |name: &str| {
        // Native routes apply only unscoped aliases, once, and reject other providers.
        config
            .aliases
            .iter()
            .find(|a| {
                a.api_shape.is_none() && (a.from == name || a.from == format!("models/{name}"))
            })
            .and_then(|a| a.to.as_deref())
            .is_none_or(|to| to.starts_with("gemini/") && native_name(to).is_some())
    };
    if let Some(requested) = requested {
        let name = native_name(requested).context("Invalid native Gemini model name")?;
        if config.mode != Mode::Client && !usable(name) {
            bail!("Selected alias routes to another provider; Gemini CLI requires a Gemini model");
        }
        return Ok(name);
    }
    if config.mode == Mode::Client {
        bail!(
            "Client relay models belong to the host; use --model with a native Gemini model or alias"
        );
    }
    let destinations = config.aliases.iter().flat_map(|a| {
        a.to.iter()
            .chain(a.reasoning_routes.values().map(|r| &r.to))
    });
    let registry = config.model_registry.iter().flat_map(|r| r.models.keys());
    let fallbacks = config
        .fallbacks
        .iter()
        .flat_map(|(source, targets)| std::iter::once(source).chain(targets));
    destinations.chain(registry).chain(fallbacks)
        .filter(|name| name.starts_with("gemini/"))
        .filter_map(|name| native_name(name))
        .find(|name| usable(name))
        .context("No Gemini model in proxy routing or model_registry; use --model with a native Gemini model")
}

fn read(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e).with_context(|| format!("Cannot read {}", path.display())),
    }
}

fn object<'a>(value: &'a mut Value, key: &str) -> Result<&'a mut Value> {
    if value.get(key).is_none() {
        value[key] = json!({});
    }
    if !value[key].is_object() {
        bail!("Gemini settings {key} must be an object; left unchanged");
    }
    Ok(&mut value[key])
}

// These environment overrides take precedence over settings.json. Remove stale
// model/auth overrides, then write only the values this setup needs.
const MANAGED_ENV: &[&str] = &[
    "GOOGLE_GEMINI_BASE_URL",
    "GEMINI_API_KEY",
    "GEMINI_API_KEY_AUTH_MECHANISM",
    "GOOGLE_GENAI_API_VERSION",
    "GEMINI_MODEL",
    "GOOGLE_GENAI_USE_VERTEXAI",
    "GOOGLE_GENAI_USE_GCA",
];

fn assignment(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    let line = line
        .strip_prefix("export")
        .filter(|rest| rest.starts_with(char::is_whitespace))
        .unwrap_or(line)
        .trim_start();
    let separator = line.find(['=', ':'])?;
    let (key, rest) = line.split_at(separator);
    let value = &rest[1..];
    if rest.starts_with(':') && !value.starts_with(char::is_whitespace) {
        return None;
    }
    let key = key.trim();
    (!key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b)))
    .then_some((key, value.trim_start()))
}

fn closing_quote(value: &str, quote: char) -> bool {
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            chars.next();
        } else if ch == quote {
            return true;
        }
    }
    false
}

fn environment(original: &str, base_url: &str, api_key: &str) -> Result<String> {
    // Proxy-generated host keys and the loopback placeholder need no escaping.
    if api_key.is_empty()
        || !api_key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        bail!("Proxy access key cannot be represented safely in Gemini .env");
    }
    let mut result = String::new();
    let mut lines = original.split_inclusive('\n');
    while let Some(line) = lines.next() {
        let mut record = line.to_owned();
        let mut managed = false;
        if let Some((key, value)) = assignment(line) {
            managed = MANAGED_ENV.contains(&key);
            if let Some(quote @ ('\'' | '"' | '`')) = value.chars().next() {
                let mut closed = closing_quote(&value[1..], quote);
                while !closed {
                    let next = lines
                        .next()
                        .context("Unclosed quoted value in Gemini .env; left unchanged")?;
                    record.push_str(next);
                    closed = closing_quote(next, quote);
                }
            }
        }
        if !managed {
            result.push_str(&record);
        }
    }
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(&format!(
        "GOOGLE_GEMINI_BASE_URL={base_url}\nGEMINI_API_KEY={api_key}\nGEMINI_API_KEY_AUTH_MECHANISM=bearer\nGOOGLE_GENAI_API_VERSION=v1beta\n"
    ));
    Ok(result)
}

pub fn configure(
    config: &Config,
    api_key: &str,
    requested: Option<&str>,
    home: Option<&Path>,
) -> Result<()> {
    let model = model(config, requested)?;
    // GEMINI_CLI_HOME overrides the user home, not the .gemini directory.
    let home = match home {
        Some(path) => path.to_owned(),
        None => std::path::PathBuf::from(
            std::env::var_os("GEMINI_CLI_HOME")
                .or_else(|| std::env::var_os("HOME"))
                .context("HOME is not set; use --gemini-home")?,
        )
        .join(".gemini"),
    };
    let settings_path = home.join("settings.json");
    let original = read(&settings_path)?;
    let mut settings: Value = if original.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(&original).context("Invalid Gemini settings.json; left unchanged")?
    };
    if !settings.is_object() {
        bail!("Gemini settings.json must be an object; left unchanged");
    }
    object(object(&mut settings, "security")?, "auth")?["selectedType"] = json!("gemini-api-key");
    object(&mut settings, "model")?["name"] = json!(model);
    let settings = serde_json::to_string_pretty(&settings)? + "\n";
    let env_path = home.join(".env");
    let base_url = format!("http://{}", config.local_address());
    let env = environment(&read(&env_path)?, &base_url, api_key)?;
    // Validate both documents before writing either, using Pi's private atomic
    // writes and backups. Repeating setup with identical content is a no-op.
    for (path, content) in [(&settings_path, &settings), (&env_path, &env)] {
        crate::rollout::write_private(path, content.as_bytes(), true)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
    }
    println!(
        "Gemini CLI configured: {} → {base_url} (model {model})",
        home.display()
    );
    println!(
        "Run gemini. Trust the workspace to load .env; for a trusted headless workspace use --skip-trust."
    );
    println!(
        "Shell variables and project .env files can override this setup. Restart running Gemini sessions."
    );
    Ok(())
}

#[cfg(test)]
mod tests;
