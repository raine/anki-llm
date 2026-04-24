use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::workspace::context::Workspace;

/// Resolve the `note-types/` root: effective workspace (cwd or configured
/// default), else `./note-types/` under cwd.
pub fn note_types_root() -> Result<PathBuf> {
    if let Some(ws) = Workspace::effective() {
        return Ok(ws.note_types_dir());
    }
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    Ok(cwd.join("note-types"))
}

/// Slugify an Anki name for use as a directory or filename.
/// Preserves ASCII alphanumerics, `-`, and `_`; replaces everything else with `_`.
/// Collapses runs of `_` and trims leading/trailing `_`.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_underscore = false;
    for c in name.chars() {
        let keep = c.is_ascii_alphanumeric() || c == '-' || c == '_';
        if keep {
            out.push(c);
            prev_underscore = c == '_';
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_preserves_safe_chars() {
        assert_eq!(slugify("Simple_name-1"), "Simple_name-1");
    }

    #[test]
    fn slugify_replaces_unsafe() {
        assert_eq!(slugify("Japanese: Vocab / v2"), "Japanese_Vocab_v2");
    }

    #[test]
    fn slugify_collapses_runs() {
        assert_eq!(slugify("a   b"), "a_b");
    }

    #[test]
    fn slugify_trims_edges() {
        assert_eq!(slugify("  hello  "), "hello");
    }

    #[test]
    fn slugify_empty_becomes_underscore() {
        assert_eq!(slugify(""), "_");
        assert_eq!(slugify("::::"), "_");
    }
}
