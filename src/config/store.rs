use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::config::error::ConfigError;

pub type PersistentConfig = BTreeMap<String, serde_json::Value>;

/// Returns the absolute path to the config file.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    let home = home::home_dir().ok_or(ConfigError::HomeDirUnavailable)?;
    Ok(home.join(".config").join("anki-llm").join("config.json"))
}

/// Reads the config file. Returns an empty map if the file does not exist.
pub fn read_config() -> Result<PersistentConfig, ConfigError> {
    let path = config_path()?;
    match fs::read_to_string(&path) {
        Ok(content) => {
            let value: serde_json::Value =
                serde_json::from_str(&content).map_err(|e| ConfigError::Parse {
                    path: path.clone(),
                    source: e,
                })?;
            match value {
                serde_json::Value::Object(map) => Ok(map.into_iter().collect()),
                _ => Err(ConfigError::NotAnObject),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PersistentConfig::new()),
        Err(e) => Err(ConfigError::Read { path, source: e }),
    }
}

/// Writes the config map to disk, creating the parent directory if needed.
pub fn write_config(config: &PersistentConfig) -> Result<(), ConfigError> {
    let path = config_path()?;
    ensure_parent_dir(&path)?;
    let json = serde_json::to_string_pretty(config).expect("config serialization should not fail");
    fs::write(&path, json).map_err(|e| ConfigError::Write { path, source: e })
}

fn ensure_parent_dir(path: &Path) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| ConfigError::CreateDir {
            path: parent.to_path_buf(),
            source: e,
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serial_test::serial;

    use super::*;

    #[test]
    #[serial]
    fn read_missing_file_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let config = read_config().unwrap();
        assert!(config.is_empty());
        unsafe { std::env::remove_var("HOME") };
    }

    #[test]
    #[serial]
    fn write_and_read_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let mut config = PersistentConfig::new();
        config.insert(
            "model".to_string(),
            serde_json::Value::String("gpt-5".to_string()),
        );
        write_config(&config).unwrap();
        let loaded = read_config().unwrap();
        assert_eq!(loaded.get("model").and_then(|v| v.as_str()), Some("gpt-5"));
        unsafe { std::env::remove_var("HOME") };
    }

    #[test]
    #[serial]
    fn rejects_non_object_config() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".config").join("anki-llm");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("config.json"), "\"just a string\"").unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let err = read_config().unwrap_err();
        assert!(matches!(err, ConfigError::NotAnObject));
        unsafe { std::env::remove_var("HOME") };
    }
}
