//! Declarative SSH trust and persisted peer routes. Private keys never leave their owner.
use super::{
    Result, configuration,
    context::{self, Context},
    replica::invalid,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

const SETTINGS: &str = "peer-sync.json";
const IDENTITY: &str = "peer-sync-identity.json";
const BEGIN: &str = "# BEGIN HEY-BOSS PEER SYNC";
const END: &str = "# END HEY-BOSS PEER SYNC";

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Identity {
    node: String,
    user: String,
    public_key: String,
    host_keys: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Peer {
    pub machine: String,
    pub address: String,
    pub identity: Identity,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Settings {
    pub controller: String,
    pub machine: String,
    pub enabled: bool,
    pub peers: Vec<Peer>,
    pub waiting: Vec<String>,
}

pub(super) fn valid_address(address: &str) -> bool {
    context::valid_host(address) && !address.contains('@')
}

fn valid_node(node: &str) -> bool {
    node.len() == 32 && node.bytes().all(|b| b.is_ascii_hexdigit())
}

fn key(text: &str) -> Result<String> {
    if text.len() > 8192 || text.contains(['\n', '\r']) {
        return Err(invalid("Invalid peer public key"));
    }
    let parts: Vec<_> = text.split_whitespace().collect();
    if parts.len() < 2
        || !matches!(
            parts[0],
            "ssh-ed25519"
                | "ssh-rsa"
                | "ecdsa-sha2-nistp256"
                | "ecdsa-sha2-nistp384"
                | "ecdsa-sha2-nistp521"
        )
        || parts[1].is_empty()
        || !parts[1]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b))
    {
        return Err(invalid("Invalid peer public key"));
    }
    Ok(format!("{} {}", parts[0], parts[1]))
}

impl Identity {
    fn validate(&self) -> Result<()> {
        if !valid_node(&self.node)
            || self.user.is_empty()
            || self.user.starts_with('-')
            || !self
                .user
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
            || self.host_keys.is_empty()
            || self.host_keys.len() > 8
        {
            return Err(invalid("Invalid peer SSH identity"));
        }
        key(&self.public_key)?;
        for host_key in &self.host_keys {
            key(host_key)?;
        }
        Ok(())
    }
}

pub(super) fn accept_identity(value: &Value, node: &str) -> Result<()> {
    let identity: Identity = serde_json::from_value(value.clone())?;
    identity.validate()?;
    if identity.node != node {
        return Err(invalid("Peer SSH identity changed machine"));
    }
    Ok(())
}

pub(super) fn desired(
    ctx: &Context,
    machine: &str,
    observed: &BTreeMap<String, Value>,
) -> Result<Value> {
    let document = if configuration::is_yaml(&ctx.desired) {
        configuration::load(ctx)?["runtime"].clone()
    } else {
        ctx.read_json(&ctx.desired, json!({}))?
    };
    plan(&document, machine, &ctx.node, observed)
}

fn plan(
    document: &Value,
    machine: &str,
    controller: &str,
    observed: &BTreeMap<String, Value>,
) -> Result<Value> {
    let enabled = document["machines"][machine]["sync_address"].is_string();
    let mut settings = Settings {
        controller: controller.into(),
        machine: machine.into(),
        enabled,
        peers: vec![],
        waiting: vec![],
    };
    if enabled {
        for (host, value) in document["machines"].as_object().into_iter().flatten() {
            let Some(address) = value["sync_address"].as_str().filter(|_| host != machine) else {
                continue;
            };
            if let Some(identity) = observed
                .get(host)
                .and_then(|m| m.get("peer_sync_identity"))
                .filter(|v| !v.is_null())
            {
                let identity: Identity = serde_json::from_value(identity.clone())?;
                identity.validate()?;
                settings.peers.push(Peer {
                    machine: host.clone(),
                    address: address.into(),
                    identity,
                });
            } else {
                settings.waiting.push(host.clone());
            }
        }
    }
    Ok(serde_json::to_value(settings)?)
}

pub(super) fn cached(ctx: &Context) -> Result<Option<Settings>> {
    let value = ctx.read_json(&ctx.state.join(SETTINGS), Value::Null)?;
    if value.is_null() {
        Ok(None)
    } else {
        Ok(Some(serde_json::from_value(value)?))
    }
}

fn checked(command: &mut Command) -> Result<Vec<u8>> {
    let output = command.output()?;
    if !output.status.success() {
        return Err(invalid(&String::from_utf8_lossy(&output.stderr)));
    }
    Ok(output.stdout)
}

fn key_path(ctx: &Context) -> std::path::PathBuf {
    ctx.home.join(".ssh/hey_boss_peer_ed25519")
}

fn identity(ctx: &Context) -> Result<Identity> {
    let path = ctx.state.join(IDENTITY);
    let cached = ctx.read_json(&path, Value::Null)?;
    if !cached.is_null() {
        let identity: Identity = serde_json::from_value(cached)?;
        identity.validate()?;
        if identity.node != ctx.node {
            return Err(invalid("Cached SSH identity belongs to another machine"));
        }
        return Ok(identity);
    }
    let private = key_path(ctx);
    ctx.protect_file(&private)?;
    ctx.protect_file(&private.with_extension("pub"))?;
    if !private.exists() {
        checked(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", "hey-boss-peer"])
                .arg("-f")
                .arg(&private),
        )?;
    }
    fs::set_permissions(&private, fs::Permissions::from_mode(0o600))?;
    let public_key = key(fs::read_to_string(private.with_extension("pub"))?.trim())?;
    let user = String::from_utf8(checked(Command::new("id").arg("-un"))?)?
        .trim()
        .to_owned();
    // Learn the actual local listener's keys (managed sshd may not use /etc/ssh keys).
    // The controller receives this advertisement through its existing trusted SSH connection.
    let output = checked(Command::new("ssh-keyscan").args([
        "-T",
        "3",
        "-t",
        "ed25519,ecdsa,rsa",
        "127.0.0.1",
    ]))?;
    let host_keys = String::from_utf8(output)?
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once(' ').map(|(_, value)| value))
        .map(key)
        .collect::<Result<Vec<_>>>()?;
    let identity = Identity {
        node: ctx.node.clone(),
        user,
        public_key,
        host_keys,
    };
    identity.validate()?;
    ctx.atomic_json(&path, &serde_json::to_value(&identity)?)?;
    Ok(identity)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn authorized(existing: &str, lines: &[String], adopted: &[String]) -> Result<String> {
    let mut result = String::new();
    let mut managed = false;
    for line in existing.lines() {
        if line == BEGIN {
            if managed {
                return Err(invalid("Malformed managed peer SSH keys"));
            }
            managed = true;
            continue;
        }
        if line == END {
            if !managed {
                return Err(invalid("Malformed managed peer SSH keys"));
            }
            managed = false;
            continue;
        }
        // Adopt the keys installed during the initial manual connectivity check.
        let manual = line
            .split_whitespace()
            .last()
            .is_some_and(|v| v.starts_with("hey-boss-peer:"))
            && adopted.iter().any(|key| line.contains(key));
        if !managed && !manual {
            result.push_str(line);
            result.push('\n');
        }
    }
    if managed {
        return Err(invalid("Unclosed managed peer SSH keys"));
    }
    if !lines.is_empty() {
        result.push_str(&format!("{BEGIN}\n{}\n{END}\n", lines.join("\n")));
    }
    Ok(result)
}

fn write_changed(ctx: &Context, path: &Path, content: &str) -> Result<()> {
    ctx.protect_file(path)?;
    if fs::read_to_string(path).ok().as_deref() != Some(content) {
        context::atomic_bytes(path, content.as_bytes())?;
    }
    Ok(())
}

pub(super) fn apply(ctx: &Context, value: &Value) -> Result<Value> {
    let settings: Settings = serde_json::from_value(value.clone())?;
    if !valid_node(&settings.controller)
        || !context::valid_host(&settings.machine)
        || settings.peers.len() > 64
    {
        return Err(invalid("Invalid peer sync configuration"));
    }
    for peer in &settings.peers {
        peer.identity.validate()?;
        if !valid_address(&peer.address)
            || !context::valid_host(&peer.machine)
            || peer.machine == settings.machine
            || peer.identity.node == ctx.node
        {
            return Err(invalid("Invalid peer sync endpoint"));
        }
    }
    let _lock = ctx.lock("peer-sync-config.lock", true)?;
    // Unconfigured machines need neither an SSH listener nor a new keypair.
    if !settings.enabled && cached(ctx)?.is_none() {
        return Ok(json!({"revision":context::hash(value),"identity":null}));
    }
    let ssh = ctx.home.join(".ssh");
    fs::create_dir_all(&ssh)?;
    fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700))?;
    let identity = if settings.enabled {
        Some(identity(ctx)?)
    } else {
        None
    };
    let command = format!(
        "exec {} fleet sync --controller {}",
        shell_quote(&ctx.binary.to_string_lossy()),
        settings.controller
    );
    let command = command.replace('\\', "\\\\").replace('"', "\\\"");
    let mut lines = vec![];
    let mut known = String::new();
    let mut adopted = vec![];
    for peer in settings.peers.iter().filter(|_| settings.enabled) {
        let public = key(&peer.identity.public_key)?;
        adopted.push(public.clone());
        lines.push(format!(
            "restrict,command=\"{command}\" {public} hey-boss-managed-peer"
        ));
        for host_key in &peer.identity.host_keys {
            known.push_str(&format!("{} {}\n", peer.identity.node, key(host_key)?));
        }
    }
    let keys = ssh.join("authorized_keys");
    ctx.protect_file(&keys)?;
    let existing = match fs::read_to_string(&keys) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    write_changed(ctx, &keys, &authorized(&existing, &lines, &adopted)?)?;
    write_changed(ctx, &ctx.state.join("peer-sync-known-hosts"), &known)?;
    ctx.atomic_json(&ctx.state.join(SETTINGS), value)?;
    Ok(json!({"revision":context::hash(value),"identity":identity}))
}

pub(super) fn ssh(ctx: &Context, peer: &Peer) -> Command {
    let mut command = Command::new("ssh");
    // This connection must not depend on forwarded agents, laptops, or host-wide proxies.
    command
        .args([
            "-F",
            "/dev/null",
            "-a",
            "-o",
            "IdentityAgent=none",
            "-o",
            "IdentitiesOnly=yes",
            "-o",
            "CertificateFile=none",
            "-o",
            "GlobalKnownHostsFile=/dev/null",
        ])
        .arg("-i")
        .arg(key_path(ctx))
        .arg("-o")
        .arg(format!("HostKeyAlias={}", peer.identity.node))
        .arg("-o")
        .arg(format!(
            "UserKnownHostsFile={}",
            ctx.state.join("peer-sync-known-hosts").display()
        ))
        .arg("-l")
        .arg(&peer.identity.user);
    command
}

pub(super) fn status(ctx: &Context) -> Result<Value> {
    let Some(settings) = cached(ctx)? else {
        return Ok(Value::Null);
    };
    let mut peers = vec![];
    for peer in &settings.peers {
        let mut value = ctx.read_json(
            &diagnostic_path(ctx, &peer.machine),
            json!({"state":if settings.machine < peer.machine { "waiting" } else { "passive" }}),
        )?;
        value["machine"] = json!(peer.machine);
        value["address"] = json!(peer.address);
        value["initiator"] = json!(settings.machine < peer.machine);
        peers.push(value);
    }
    Ok(json!({"enabled":settings.enabled,"waiting":settings.waiting,"peers":peers}))
}

pub(super) fn diagnostic_path(ctx: &Context, machine: &str) -> std::path::PathBuf {
    ctx.state
        .join(format!("peer-sync-{}.json", context::hash(&json!(machine))))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn peer_provisioning_survives_restart_and_revokes_removed_routes() {
        let root = crate::admin::Temporary::new().unwrap();
        let ctx = Context {
            home: root.0.clone(),
            state: root.0.join("state"),
            desired: root.0.join("fleet.yaml"),
            path: root.0.join("issues.db"),
            binary: root.0.join("path with spaces/hey-boss"),
            node: "a".repeat(32),
            stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        fs::create_dir_all(&ctx.state).unwrap();
        fs::create_dir_all(ctx.home.join(".ssh")).unwrap();
        ctx.atomic_json(&ctx.state.join(IDENTITY), &identity("a"))
            .unwrap();
        fs::write(
            ctx.home.join(".ssh/authorized_keys"),
            "ssh-ed25519 USER user-access\n",
        )
        .unwrap();
        let settings = json!({"controller":"c".repeat(32),"machine":"a","enabled":true,"waiting":[],"peers":[{"machine":"b","address":"b.internal","identity":identity("b")}]});
        apply(&ctx, &settings).unwrap();
        let keys = fs::read_to_string(ctx.home.join(".ssh/authorized_keys")).unwrap();
        assert!(keys.starts_with("ssh-ed25519 USER user-access\n"));
        assert!(keys.contains("restrict,command=\"exec '"));
        assert!(keys.contains(&format!("fleet sync --controller {}", "c".repeat(32))));
        assert_eq!(cached(&ctx).unwrap().unwrap().controller, "c".repeat(32));
        assert_eq!(
            cached(&ctx).unwrap().unwrap().peers[0].address,
            "b.internal"
        );
        apply(&ctx, &settings).unwrap();
        assert_eq!(
            fs::read_to_string(ctx.home.join(".ssh/authorized_keys")).unwrap(),
            keys
        );
        apply(&ctx,&json!({"controller":"c".repeat(32),"machine":"a","enabled":false,"waiting":[],"peers":[]})).unwrap();
        assert_eq!(
            fs::read_to_string(ctx.home.join(".ssh/authorized_keys")).unwrap(),
            "ssh-ed25519 USER user-access\n"
        );
        assert!(cached(&ctx).unwrap().unwrap().peers.is_empty());
        assert_eq!(
            fs::metadata(ctx.home.join(".ssh/authorized_keys"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    fn identity(node: &str) -> Value {
        json!({"node":node.repeat(32),"user":"worker","public_key":"ssh-ed25519 QUJD","host_keys":["ssh-rsa REVG"]})
    }
    #[test]
    fn peer_plan_waits_for_trusted_identities_and_uses_one_initiator() {
        let doc = json!({"machines":{"a":{"sync_address":"a.internal"},"b":{"sync_address":"b.internal"},"local":{}}});
        let mut observed = BTreeMap::new();
        let first = plan(&doc, "a", &"c".repeat(32), &observed).unwrap();
        assert_eq!(first["waiting"], json!(["b"]));
        observed.insert("b".into(), json!({"peer_sync_identity":identity("b")}));
        let ready: Settings =
            serde_json::from_value(plan(&doc, "a", &"c".repeat(32), &observed).unwrap()).unwrap();
        assert_eq!(ready.peers.len(), 1);
        assert!(ready.machine < ready.peers[0].machine);
        assert_eq!(ready.controller, "c".repeat(32));
        assert_eq!(
            plan(&doc, "local", &"c".repeat(32), &observed).unwrap()["enabled"],
            false
        );
    }
    #[test]
    fn peer_keys_are_replaced_and_revoked_without_touching_user_access() {
        let original = "ssh-ed25519 USER user\nrestrict ssh-ed25519 OLD hey-boss-peer:manual\n";
        let first = authorized(
            original,
            &["restrict,command=\"sync\" ssh-ed25519 OLD managed".into()],
            &["ssh-ed25519 OLD".into()],
        )
        .unwrap();
        assert!(first.starts_with("ssh-ed25519 USER user\n"));
        assert!(!first.contains("hey-boss-peer:manual"));
        assert_eq!(
            authorized(&first, &[], &[]).unwrap(),
            "ssh-ed25519 USER user\n"
        );
        assert!(authorized(BEGIN, &[], &[]).is_err());
    }
    #[test]
    fn peer_identity_cannot_inject_ssh_options_or_change_machine() {
        assert!(accept_identity(&identity("a"), &"b".repeat(32)).is_err());
        let mut id = identity("a");
        id["user"] = json!("-option");
        assert!(accept_identity(&id, &"a".repeat(32)).is_err());
        assert!(key("ssh-ed25519 AAAA\ncommand=bad").is_err());
        assert!(!valid_address("user@host"));
    }
}
