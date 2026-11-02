//! Editable SSH inventory shared by discovery and the automatic connector.
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub fn path(directory: &Path) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    let path = directory.join("config.yaml");
    let old = path.with_extension("json");
    if !path.exists() && old.exists() {
        let value: Value = serde_json::from_slice(&fs::read(&old)?)?;
        let temporary = path.with_extension(format!("{}.new", crate::issues::worker::random_id()?));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        use std::io::Write;
        file.write_all(serde_yaml_ng::to_string(&value)?.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, &path)?;
        fs::remove_file(old)?;
    }
    Ok(path)
}

pub fn read(path: &Path) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_yaml_ng::from_slice(&bytes)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_inventory_without_a_second_copy_and_keeps_yaml_comments() {
        let home = std::env::temp_dir().join(format!(
            "hey-boss-config-{}",
            crate::issues::worker::random_id().unwrap()
        ));
        fs::create_dir_all(home.join(".hey-boss")).unwrap();
        let old = home.join(".hey-boss/config.json");
        fs::write(&old, r#"{"ssh_hosts":["devbox"]}"#).unwrap();
        let path = path(&home.join(".hey-boss")).unwrap();
        assert!(!old.exists());
        assert_eq!(read(&path).unwrap()["ssh_hosts"][0], "devbox");
        let text = "# machines\nssh_hosts:\n  - devbox\n  # - paused.local\n";
        fs::write(&path, text).unwrap();
        assert_eq!(
            read(&path).unwrap()["ssh_hosts"].as_array().unwrap().len(),
            1
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        fs::remove_dir_all(home).unwrap();
    }
}
