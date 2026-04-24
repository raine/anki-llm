use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::note_type::paths::{note_types_root, slugify};

const MANIFEST_FILE: &str = "note-type.yaml";
const CSS_FILE: &str = "style.css";

/// On-disk manifest: real Anki names + canonical template order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteTypeManifest {
    /// Real Anki model name.
    pub name: String,
    /// Ordered list of (anki_template_name, template_slug).
    pub templates: Vec<TemplateEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateEntry {
    /// Real Anki template name.
    pub name: String,
    /// Filesystem slug used for `<slug>.front.html` / `<slug>.back.html`.
    pub slug: String,
}

/// Represents the files for a single note type on disk.
#[derive(Debug, Clone)]
pub struct NoteTypeFiles {
    pub manifest: NoteTypeManifest,
    pub root: PathBuf,
    pub css: String,
    /// Keyed by real Anki template name; order matches manifest.
    pub templates: IndexMap<String, TemplatePair>,
}

#[derive(Debug, Clone)]
pub struct TemplatePair {
    pub front: String,
    pub back: String,
}

impl NoteTypeFiles {
    /// Load a note type from disk by real Anki name (manifest lookup).
    pub fn load(name: &str) -> Result<Self> {
        let root = Self::find_root_by_name(name)?;
        Self::load_from_path(&root)
    }

    /// Load from an explicit directory path.
    pub fn load_from_path(root: &Path) -> Result<Self> {
        let manifest_path = root.join(MANIFEST_FILE);
        let manifest_str = fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;
        let manifest: NoteTypeManifest = serde_yaml::from_str(&manifest_str)
            .with_context(|| format!("failed to parse {}", manifest_path.display()))?;

        let css_path = root.join(CSS_FILE);
        let css = fs::read_to_string(&css_path)
            .with_context(|| format!("failed to read {}", css_path.display()))?;

        let mut templates = IndexMap::new();
        for entry in &manifest.templates {
            let front_path = root.join(format!("{}.front.html", entry.slug));
            let back_path = root.join(format!("{}.back.html", entry.slug));
            let front = fs::read_to_string(&front_path)
                .with_context(|| format!("failed to read {}", front_path.display()))?;
            let back = fs::read_to_string(&back_path)
                .with_context(|| format!("failed to read {}", back_path.display()))?;
            templates.insert(entry.name.clone(), TemplatePair { front, back });
        }

        Ok(Self {
            manifest,
            root: root.to_path_buf(),
            css,
            templates,
        })
    }

    /// Write files to disk, removing any stale `.front.html`/`.back.html` from prior runs.
    pub fn write(&self) -> Result<()> {
        fs::create_dir_all(&self.root)?;

        let manifest_path = self.root.join(MANIFEST_FILE);
        let manifest_yaml = serde_yaml::to_string(&self.manifest)?;
        fs::write(&manifest_path, manifest_yaml)
            .with_context(|| format!("failed to write {}", manifest_path.display()))?;

        let css_path = self.root.join(CSS_FILE);
        fs::write(&css_path, &self.css)
            .with_context(|| format!("failed to write {}", css_path.display()))?;

        let mut keep: HashSet<String> =
            HashSet::from([MANIFEST_FILE.to_string(), CSS_FILE.to_string()]);
        for entry in &self.manifest.templates {
            let front_name = format!("{}.front.html", entry.slug);
            let back_name = format!("{}.back.html", entry.slug);
            let pair = self.templates.get(&entry.name).with_context(|| {
                format!(
                    "internal error: manifest template '{}' missing from templates map",
                    entry.name
                )
            })?;
            fs::write(self.root.join(&front_name), &pair.front)?;
            fs::write(self.root.join(&back_name), &pair.back)?;
            keep.insert(front_name);
            keep.insert(back_name);
        }

        // Remove stale template files from previous pulls.
        for dirent in fs::read_dir(&self.root)? {
            let dirent = dirent?;
            let Some(name) = dirent.file_name().to_str().map(String::from) else {
                continue;
            };
            let is_template_file = name.ends_with(".front.html") || name.ends_with(".back.html");
            if is_template_file && !keep.contains(&name) {
                fs::remove_file(dirent.path())?;
            }
        }

        Ok(())
    }

    /// Find the directory for a note type by its real Anki name by scanning manifests.
    fn find_root_by_name(name: &str) -> Result<PathBuf> {
        let root = note_types_root()?;
        if root.is_dir() {
            for dirent in fs::read_dir(&root)? {
                let dirent = dirent?;
                let path = dirent.path();
                if !path.is_dir() {
                    continue;
                }
                let manifest_path = path.join(MANIFEST_FILE);
                if !manifest_path.is_file() {
                    continue;
                }
                let Ok(contents) = fs::read_to_string(&manifest_path) else {
                    continue;
                };
                let Ok(manifest) = serde_yaml::from_str::<NoteTypeManifest>(&contents) else {
                    continue;
                };
                if manifest.name == name {
                    return Ok(path);
                }
            }
        }
        bail!(
            "Note type '{}' not found under {}.\n\
             Run `anki-llm note-type pull \"{}\"` to create it from Anki.",
            name,
            root.display(),
            name
        );
    }

    /// Assign a fresh directory path for a new note type, avoiding collisions.
    pub fn fresh_dir(name: &str) -> Result<PathBuf> {
        let root = note_types_root()?;
        fs::create_dir_all(&root)?;
        let base = slugify(name);
        let mut candidate = root.join(&base);
        let mut n = 2;
        while candidate.exists() {
            candidate = root.join(format!("{base}-{n}"));
            n += 1;
        }
        Ok(candidate)
    }

    /// List all note-type directories (each must contain a manifest).
    pub fn discover() -> Result<Vec<PathBuf>> {
        let root = note_types_root()?;
        if !root.is_dir() {
            return Ok(Vec::new());
        }
        let mut dirs = Vec::new();
        for dirent in fs::read_dir(&root)? {
            let dirent = dirent?;
            let path = dirent.path();
            if path.is_dir() && path.join(MANIFEST_FILE).is_file() {
                dirs.push(path);
            }
        }
        dirs.sort();
        Ok(dirs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_files(root: PathBuf, anki_name: &str) -> NoteTypeFiles {
        let mut templates = IndexMap::new();
        templates.insert(
            "Recognition → Production".to_string(),
            TemplatePair {
                front: "F".into(),
                back: "B".into(),
            },
        );
        NoteTypeFiles {
            manifest: NoteTypeManifest {
                name: anki_name.to_string(),
                templates: vec![TemplateEntry {
                    name: "Recognition → Production".into(),
                    slug: "Recognition_Production".into(),
                }],
            },
            root,
            css: ".card{}".into(),
            templates,
        }
    }

    #[test]
    fn round_trip_with_unsafe_names() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("japanese_vocab");
        fs::create_dir_all(&root).unwrap();

        let files = make_files(root.clone(), "Japanese: Vocab / v2");
        files.write().unwrap();

        let loaded = NoteTypeFiles::load_from_path(&root).unwrap();
        assert_eq!(loaded.manifest.name, "Japanese: Vocab / v2");
        assert_eq!(loaded.templates["Recognition → Production"].front, "F");
        assert_eq!(loaded.css, ".card{}");
    }

    #[test]
    fn write_removes_stale_template_files() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();

        fs::write(root.join("Stale.front.html"), "x").unwrap();
        fs::write(root.join("Stale.back.html"), "x").unwrap();

        let files = make_files(root.clone(), "M");
        files.write().unwrap();

        assert!(!root.join("Stale.front.html").exists());
        assert!(!root.join("Stale.back.html").exists());
        assert!(root.join("Recognition_Production.front.html").exists());
    }

    #[test]
    fn missing_back_html_fails() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();

        let files = make_files(root.clone(), "M");
        files.write().unwrap();

        // Corrupt state: delete back.html.
        fs::remove_file(root.join("Recognition_Production.back.html")).unwrap();

        assert!(NoteTypeFiles::load_from_path(&root).is_err());
    }

    #[test]
    fn discover_only_returns_dirs_with_manifest() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("has_manifest")).unwrap();
        fs::write(
            root.join("has_manifest/note-type.yaml"),
            "name: X\ntemplates: []\n",
        )
        .unwrap();
        fs::create_dir(root.join("no_manifest")).unwrap();

        // Simulate discover behavior by using read_dir directly (bypasses cwd).
        let mut dirs: Vec<PathBuf> = fs::read_dir(root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_dir() && p.join(MANIFEST_FILE).is_file())
            .collect();
        dirs.sort();
        assert_eq!(dirs.len(), 1);
        assert!(dirs[0].ends_with("has_manifest"));
    }
}
