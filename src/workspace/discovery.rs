use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

/// Metadata extracted from a prompt file's frontmatter for display in the picker.
#[derive(Debug, Clone)]
pub struct PromptEntry {
    /// Absolute path to the prompt file.
    pub path: PathBuf,
    /// Human-readable title (from frontmatter `title`, or filename stem).
    pub title: String,
    /// Optional description from frontmatter.
    pub description: Option<String>,
    /// Deck name from frontmatter.
    pub deck: Option<String>,
    /// Note type from frontmatter.
    pub note_type: Option<String>,
}

/// Quick frontmatter-only metadata extracted without full validation.
#[derive(Debug, Default, serde::Deserialize)]
struct QuickFrontmatter {
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    deck: Option<String>,
    #[serde(default)]
    note_type: Option<String>,
}

/// Scan a directory for `.md` prompt files and extract display metadata.
///
/// Skips files with missing or unparseable frontmatter (logs to stderr).
/// Returns entries sorted by title.
pub fn discover_prompts(dir: &Path) -> Vec<PromptEntry> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!(
                "Warning: could not read prompts directory {}: {e}",
                dir.display()
            );
            return Vec::new();
        }
    };

    let re = Regex::new(r"(?s)^---\s*\n(.*?)\n---").unwrap();
    let mut prompts = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("Warning: could not read {}: {e}", path.display());
                continue;
            }
        };

        let Some(caps) = re.captures(&content) else {
            eprintln!(
                "Warning: skipping {} (no frontmatter found)",
                path.display()
            );
            continue;
        };

        let yaml_text = &caps[1];
        let meta: QuickFrontmatter = match serde_yaml::from_str(yaml_text) {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "Warning: skipping {} (invalid frontmatter: {e})",
                    path.display()
                );
                continue;
            }
        };

        let title = meta.title.unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        });

        prompts.push(PromptEntry {
            path: path.canonicalize().unwrap_or(path),
            title,
            description: meta.description,
            deck: meta.deck,
            note_type: meta.note_type,
        });
    }

    prompts.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    prompts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_md_files_with_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        let prompt = tmp.path().join("test.md");
        fs::write(
            &prompt,
            "---\ntitle: My Prompt\ndeck: Test\nnote_type: Basic\n---\n\nbody",
        )
        .unwrap();
        // Non-md file should be ignored
        fs::write(tmp.path().join("readme.txt"), "not a prompt").unwrap();

        let entries = discover_prompts(tmp.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "My Prompt");
        assert_eq!(entries[0].deck.as_deref(), Some("Test"));
    }

    #[test]
    fn falls_back_to_filename_when_no_title() {
        let tmp = tempfile::tempdir().unwrap();
        let prompt = tmp.path().join("japanese-vocab.md");
        fs::write(
            &prompt,
            "---\ndeck: Japanese\nnote_type: Basic\n---\n\nbody",
        )
        .unwrap();

        let entries = discover_prompts(tmp.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "japanese-vocab");
    }

    #[test]
    fn skips_malformed_frontmatter() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("bad.md"), "no frontmatter here").unwrap();
        fs::write(
            tmp.path().join("good.md"),
            "---\ndeck: Test\nnote_type: Basic\n---\n\nbody",
        )
        .unwrap();

        let entries = discover_prompts(tmp.path());
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "good");
    }

    #[test]
    fn empty_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let entries = discover_prompts(tmp.path());
        assert!(entries.is_empty());
    }
}
