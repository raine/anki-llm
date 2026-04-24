use std::path::{Path, PathBuf};

use crate::workspace::manifest::{MANIFEST_FILE_NAME, WorkspaceManifest, read_manifest};

/// A workspace is simply the current directory when it contains
/// an `anki-llm.yaml` file or a `prompts/` subdirectory.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf,
    pub manifest: WorkspaceManifest,
}

impl Workspace {
    /// Check if `dir` is a workspace (has `anki-llm.yaml` or `prompts/`).
    pub fn in_dir(dir: &Path) -> Option<Self> {
        let manifest_path = dir.join(MANIFEST_FILE_NAME);
        let prompts_dir = dir.join("prompts");

        if !manifest_path.is_file() && !prompts_dir.is_dir() {
            return None;
        }

        let manifest = if manifest_path.is_file() {
            read_manifest(&manifest_path).ok().unwrap_or_default()
        } else {
            WorkspaceManifest::default()
        };

        Some(Self {
            root: dir.to_path_buf(),
            manifest,
        })
    }

    pub fn prompts_dir(&self) -> PathBuf {
        self.root.join("prompts")
    }

    pub fn note_types_dir(&self) -> PathBuf {
        self.root.join("note-types")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn workspace_with_manifest() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("anki-llm.yaml"), "default_model: gpt-5\n").unwrap();

        let ws = Workspace::in_dir(dir.path()).unwrap();
        assert_eq!(ws.root, dir.path());
        assert_eq!(ws.manifest.default_model, Some("gpt-5".into()));
    }

    #[test]
    fn workspace_with_prompts_dir_only() {
        let dir = tempdir().unwrap();
        std::fs::create_dir(dir.path().join("prompts")).unwrap();

        let ws = Workspace::in_dir(dir.path()).unwrap();
        assert_eq!(ws.root, dir.path());
        assert!(ws.manifest.default_model.is_none());
    }

    #[test]
    fn no_workspace_when_empty() {
        let dir = tempdir().unwrap();
        assert!(Workspace::in_dir(dir.path()).is_none());
    }
}
