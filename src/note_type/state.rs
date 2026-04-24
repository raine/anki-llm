use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::anki::schema::CardTemplate;

const STATE_FILE: &str = ".sync-state.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncState {
    /// SHA-256 over (css + ordered templates) of the remote state at last sync.
    pub last_remote_hash: String,
}

/// Compute a stable hash over CSS + ordered templates.
pub fn hash_remote(css: &str, templates: &IndexMap<String, CardTemplate>) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"css\0");
    hasher.update(css.as_bytes());
    for (name, tmpl) in templates {
        hasher.update(b"tmpl\0");
        hasher.update(name.as_bytes());
        hasher.update(b"\0front\0");
        hasher.update(tmpl.front.as_bytes());
        hasher.update(b"\0back\0");
        hasher.update(tmpl.back.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

pub fn read(root: &Path) -> Result<Option<SyncState>> {
    let path = root.join(STATE_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let contents =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let state: SyncState = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(state))
}

pub fn write(root: &Path, state: &SyncState) -> Result<()> {
    let path = root.join(STATE_FILE);
    let json = serde_json::to_string_pretty(state)?;
    fs::write(&path, json).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

/// Ensure `.sync-state.json` is listed in `<root>/.gitignore`. Idempotent.
pub fn ensure_gitignored(root: &Path) -> Result<()> {
    let path = root.join(".gitignore");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == STATE_FILE) {
        return Ok(());
    }
    let mut new = existing;
    if !new.is_empty() && !new.ends_with('\n') {
        new.push('\n');
    }
    new.push_str(STATE_FILE);
    new.push('\n');
    fs::write(&path, new)?;
    Ok(())
}
