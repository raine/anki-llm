use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::error::ConfigError;

/// Typed application configuration stored at `~/.config/anki-llm/config.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nerd_font: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompts_dir: Option<PathBuf>,
    /// Custom API base URL (e.g. OpenRouter, Ollama, or any OpenAI-compatible endpoint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_base_url: Option<String>,
}

impl AppConfig {
    /// Get a config value by key name (for `config get`).
    pub fn get(&self, key: &str) -> Option<String> {
        match key {
            "model" => self.model.clone(),
            "nerd_font" => self.nerd_font.map(|b| b.to_string()),
            "prompts_dir" => self.prompts_dir.as_ref().map(|p| p.display().to_string()),
            "api_base_url" => self.api_base_url.clone(),
            _ => None,
        }
    }

    /// Set a config value by key name (for `config set`). Returns true if key is known.
    pub fn set(&mut self, key: &str, value: &str) -> bool {
        match key {
            "model" => {
                self.model = Some(value.to_string());
                true
            }
            "nerd_font" => {
                self.nerd_font = Some(value != "false" && value != "0");
                true
            }
            "prompts_dir" => {
                self.prompts_dir = Some(PathBuf::from(value));
                true
            }
            "api_base_url" => {
                self.api_base_url = Some(value.to_string());
                true
            }
            _ => false,
        }
    }

    /// List all set key-value pairs (for `config list`).
    pub fn entries(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Some(ref v) = self.model {
            out.push(("model".into(), v.clone()));
        }
        if let Some(v) = self.nerd_font {
            out.push(("nerd_font".into(), v.to_string()));
        }
        if let Some(ref v) = self.prompts_dir {
            out.push(("prompts_dir".into(), v.display().to_string()));
        }
        if let Some(ref v) = self.api_base_url {
            out.push(("api_base_url".into(), v.clone()));
        }
        out
    }
}

/// Ephemeral application state stored at `~/.local/state/anki-llm/state.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppState {
    /// Last-used prompt path (absolute).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_prompt: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Config I/O
// ---------------------------------------------------------------------------

/// Returns the absolute path to the config file.
pub fn config_path() -> Result<PathBuf, ConfigError> {
    let home = home::home_dir().ok_or(ConfigError::HomeDirUnavailable)?;
    Ok(home.join(".config").join("anki-llm").join("config.json"))
}

/// Reads the typed config. Returns defaults if the file does not exist.
pub fn read_config() -> Result<AppConfig, ConfigError> {
    let path = config_path()?;
    match fs::read_to_string(&path) {
        Ok(content) => {
            let config: AppConfig =
                serde_json::from_str(&content).map_err(|e| ConfigError::Parse {
                    path: path.clone(),
                    source: e,
                })?;
            Ok(config)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppConfig::default()),
        Err(e) => Err(ConfigError::Read { path, source: e }),
    }
}

/// Writes the config to disk, creating the parent directory if needed.
pub fn write_config(config: &AppConfig) -> Result<(), ConfigError> {
    let path = config_path()?;
    ensure_parent_dir(&path)?;
    let json = serde_json::to_string_pretty(config).expect("config serialization should not fail");
    fs::write(&path, json).map_err(|e| ConfigError::Write { path, source: e })
}

// ---------------------------------------------------------------------------
// State I/O
// ---------------------------------------------------------------------------

/// Returns the absolute path to the state file.
pub fn state_path() -> Result<PathBuf, ConfigError> {
    let home = home::home_dir().ok_or(ConfigError::HomeDirUnavailable)?;
    Ok(home
        .join(".local")
        .join("state")
        .join("anki-llm")
        .join("state.json"))
}

/// Reads ephemeral app state. Returns defaults if the file does not exist.
pub fn read_state() -> Result<AppState, ConfigError> {
    let path = state_path()?;
    match fs::read_to_string(&path) {
        Ok(content) => {
            let state: AppState =
                serde_json::from_str(&content).map_err(|e| ConfigError::Parse {
                    path: path.clone(),
                    source: e,
                })?;
            Ok(state)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AppState::default()),
        Err(e) => Err(ConfigError::Read { path, source: e }),
    }
}

/// Writes ephemeral app state to disk.
pub fn write_state(state: &AppState) -> Result<(), ConfigError> {
    let path = state_path()?;
    ensure_parent_dir(&path)?;
    let json = serde_json::to_string_pretty(state).expect("state serialization should not fail");
    fs::write(&path, json).map_err(|e| ConfigError::Write { path, source: e })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
    fn read_missing_file_returns_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let config = read_config().unwrap();
        assert!(config.model.is_none());
        assert!(config.prompts_dir.is_none());
        unsafe { std::env::remove_var("HOME") };
    }

    #[test]
    #[serial]
    fn write_and_read_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let mut config = AppConfig::default();
        config.model = Some("gpt-5".into());
        write_config(&config).unwrap();
        let loaded = read_config().unwrap();
        assert_eq!(loaded.model.as_deref(), Some("gpt-5"));
        unsafe { std::env::remove_var("HOME") };
    }

    #[test]
    #[serial]
    fn backward_compat_with_old_json() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".config").join("anki-llm");
        std::fs::create_dir_all(&dir).unwrap();
        // Old-style config with string values
        std::fs::write(
            dir.join("config.json"),
            r#"{"model": "gpt-5", "nerd_font": true}"#,
        )
        .unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let config = read_config().unwrap();
        assert_eq!(config.model.as_deref(), Some("gpt-5"));
        assert_eq!(config.nerd_font, Some(true));
        unsafe { std::env::remove_var("HOME") };
    }

    #[test]
    fn get_set_keys() {
        let mut config = AppConfig::default();
        assert!(config.set("model", "gpt-5"));
        assert_eq!(config.get("model"), Some("gpt-5".into()));
        assert!(config.set("nerd_font", "false"));
        assert_eq!(config.get("nerd_font"), Some("false".into()));
        assert!(config.set("prompts_dir", "/tmp/prompts"));
        assert_eq!(config.get("prompts_dir"), Some("/tmp/prompts".into()));
        assert!(!config.set("unknown_key", "value"));
        assert!(config.get("unknown_key").is_none());
    }

    #[test]
    #[serial]
    fn state_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let mut state = AppState::default();
        state.last_prompt = Some(PathBuf::from("/tmp/test.md"));
        write_state(&state).unwrap();
        let loaded = read_state().unwrap();
        assert_eq!(
            loaded.last_prompt.as_deref(),
            Some(Path::new("/tmp/test.md"))
        );
        unsafe { std::env::remove_var("HOME") };
    }
}
