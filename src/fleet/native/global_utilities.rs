//! Global launchers are owned symlinks; unrelated commands are never replaced.
use super::{
    Result, configuration,
    context::{self, Context},
    replica::invalid,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink},
    path::Path,
};

pub(super) fn desired(ctx: &Context) -> Result<Value> {
    if !configuration::is_yaml(&ctx.desired) {
        return Ok(json!([]));
    }
    let config = configuration::load(ctx)?;
    Ok(json!(
        config["document"]["utils"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, definition)| definition["global"] == true)
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
    ))
}

// Report failures without interrupting fleet or worker sync.
pub(super) fn apply(ctx: &Context, names: &Value) -> Value {
    match reconcile(ctx, names) {
        Ok(errors) if errors.is_empty() => Value::Null,
        Ok(errors) => json!(errors.join("; ")),
        Err(error) => json!(error.to_string()),
    }
}

pub(super) fn reconcile(ctx: &Context, names: &Value) -> Result<Vec<String>> {
    let names: BTreeSet<String> = serde_json::from_value(names.clone())?;
    if names
        .iter()
        .any(|name| !crate::utilities::valid_name(name) || name == "hey-boss")
    {
        return Err(invalid("Invalid global utility name"));
    }
    let _lock = ctx.lock("global-utilities.lock", true)?;
    let directory = ctx.state.join("global-utilities");
    if names.is_empty() && !directory.exists() {
        return Ok(vec![]);
    }
    let bin = ctx.home.join(".local/bin");
    fs::create_dir_all(&directory)?;
    fs::create_dir_all(&bin)?;
    let mut errors = vec![];
    for name in &names {
        let result = (|| -> Result<()> {
            let link = bin.join(name);
            let target = directory.join(name);
            match fs::symlink_metadata(&link) {
                Ok(_) if fs::read_link(&link).ok().as_ref() == Some(&target) => {}
                Ok(_) => {
                    return Err(invalid(&format!(
                        "{} already exists; left unchanged",
                        link.display()
                    )));
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            let quote = |text: &str| format!("'{}'", text.replace('\'', "'\\''"));
            let binary = ctx
                .binary
                .to_str()
                .ok_or_else(|| invalid("Utility launcher path must be UTF-8"))?;
            let script = format!(
                "#!/bin/sh\n# Managed by hey-boss: global utility\nexec {} utils {} \"$@\"\n",
                quote(binary),
                quote(name)
            );
            if fs::read(&target).ok().as_deref() != Some(script.as_bytes()) {
                write_executable(&target, script.as_bytes())?;
            } else if fs::metadata(&target)?.permissions().mode() & 0o111 != 0o111 {
                fs::set_permissions(&target, fs::Permissions::from_mode(0o755))?;
            }
            if fs::read_link(&link).ok().as_ref() != Some(&target) {
                symlink(&target, &link)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            errors.push(format!("{name}: {error}"));
        }
    }
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name
            .to_str()
            .filter(|name| crate::utilities::valid_name(name))
        else {
            continue;
        };
        if names.contains(name) {
            continue;
        }
        let link = bin.join(name);
        if fs::read_link(&link).ok().as_ref() == Some(&entry.path()) {
            fs::remove_file(&link)?;
        }
        fs::remove_file(entry.path())?;
    }
    Ok(errors)
}

fn write_executable(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = path.with_extension(format!("{}.new", context::id()?));
    let result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    let _ = fs::remove_file(temporary);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        process::Command,
    };

    #[test]
    fn global_utilities_forward_arguments_update_and_remove_only_owned_links() {
        let (root, mut ctx, store) = super::super::context::tests::test_context();
        ctx.binary = root.join("hey boss's cli");
        fs::write(&ctx.binary, "#!/bin/sh\nprintf '%s\\0' \"$@\"\nexit 37\n").unwrap();
        fs::set_permissions(&ctx.binary, fs::Permissions::from_mode(0o755)).unwrap();
        let names = json!(["ans-ck", "ans-cat"]);
        assert!(reconcile(&ctx, &names).unwrap().is_empty());
        let bin = ctx.home.join(".local/bin");
        let output = Command::new(bin.join("ans-ck"))
            .args([
                "--sem",
                "agent API authentication",
                "",
                "$(echo injected)",
                "--",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(37));
        assert_eq!(
            output.stdout,
            b"utils\0ans-ck\0--sem\0agent API authentication\0\0$(echo injected)\0--\0"
        );
        let target = fs::read_link(bin.join("ans-ck")).unwrap();
        let modified = fs::metadata(&target).unwrap().modified().unwrap();
        assert!(reconcile(&ctx, &names).unwrap().is_empty());
        assert_eq!(fs::metadata(&target).unwrap().modified().unwrap(), modified);
        let replacement = root.join("new-cli");
        fs::rename(&ctx.binary, &replacement).unwrap();
        ctx.binary = replacement;
        reconcile(&ctx, &names).unwrap();
        assert_eq!(
            Command::new(bin.join("ans-ck"))
                .arg("--help")
                .output()
                .unwrap()
                .stdout,
            b"utils\0ans-ck\0--help\0"
        );
        // A user replacement must survive removing the configuration.
        fs::remove_file(bin.join("ans-cat")).unwrap();
        fs::write(bin.join("ans-cat"), "user command").unwrap();
        reconcile(&ctx, &json!([])).unwrap();
        assert!(!bin.join("ans-ck").symlink_metadata().is_ok());
        assert_eq!(
            fs::read_to_string(bin.join("ans-cat")).unwrap(),
            "user command"
        );
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn global_utilities_preserve_collisions_and_reject_unsafe_names() {
        let (root, ctx, store) = super::super::context::tests::test_context();
        let bin = ctx.home.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("ans-cat"), "existing").unwrap();
        symlink("missing-target", bin.join("ans-ls")).unwrap();
        let errors = reconcile(&ctx, &json!(["ans-cat", "ans-ls", "ans-ck"])).unwrap();
        assert_eq!(errors.len(), 2);
        assert_eq!(fs::read_to_string(bin.join("ans-cat")).unwrap(), "existing");
        assert_eq!(
            fs::read_link(bin.join("ans-ls")).unwrap().to_str(),
            Some("missing-target")
        );
        assert!(bin.join("ans-ck").is_file());
        for names in [
            json!(["../escape"]),
            json!(["hey-boss"]),
            json!(["--help"]),
            json!({}),
        ] {
            assert!(reconcile(&ctx, &names).is_err());
        }
        // A later retry installs a previously conflicting name.
        fs::remove_file(bin.join("ans-cat")).unwrap();
        assert!(reconcile(&ctx, &json!(["ans-cat"])).unwrap().is_empty());
        assert!(bin.join("ans-cat").is_file());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}
