use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const MANIFEST_FILE_NAME: &str = "anki-llm.yaml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct WorkspaceManifest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
}

pub fn read_manifest(path: &Path) -> Result<WorkspaceManifest> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let manifest: WorkspaceManifest = serde_yaml::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(manifest)
}

pub fn write_manifest(path: &Path, manifest: &WorkspaceManifest) -> Result<()> {
    let yaml = serde_yaml::to_string(manifest).context("failed to serialize workspace manifest")?;
    std::fs::write(path, yaml).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("anki-llm.yaml");
        let manifest = WorkspaceManifest {
            default_model: Some("gpt-5".into()),
        };
        write_manifest(&path, &manifest).unwrap();
        let loaded = read_manifest(&path).unwrap();
        assert_eq!(loaded.default_model, Some("gpt-5".into()));
    }

    #[test]
    fn default_manifest() {
        let m = WorkspaceManifest::default();
        assert!(m.default_model.is_none());
    }
}
