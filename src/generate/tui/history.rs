use std::fs;
use std::io::Write;
use std::path::PathBuf;

pub(super) const HISTORY_MAX: usize = 100;

pub(super) struct InputHistory {
    entries: Vec<String>,
    /// Index into entries (0 = most recent). `None` = not browsing history.
    cursor: Option<usize>,
    /// Text the user was typing before they started browsing history.
    stashed: String,
}

impl InputHistory {
    pub(super) fn load() -> Self {
        let mut entries = Self::path()
            .and_then(|p| fs::read_to_string(p).ok())
            .map(|s| s.lines().rev().map(String::from).collect::<Vec<_>>())
            .unwrap_or_default();
        entries.truncate(HISTORY_MAX);
        InputHistory {
            entries,
            cursor: None,
            stashed: String::new(),
        }
    }

    fn path() -> Option<PathBuf> {
        home::home_dir().map(|h| {
            h.join(".local")
                .join("state")
                .join("anki-llm")
                .join("history")
        })
    }

    pub(super) fn push(&mut self, term: &str) {
        // Don't add duplicate of most recent entry
        if self.entries.first().is_some_and(|e| e == term) {
            return;
        }
        self.entries.insert(0, term.to_string());
        self.entries.truncate(HISTORY_MAX);
        self.save();
    }

    fn save(&self) {
        let Some(path) = Self::path() else { return };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        let tmp = path.with_extension("tmp");
        let ok = (|| -> std::io::Result<()> {
            let mut f = std::io::BufWriter::new(fs::File::create(&tmp)?);
            for entry in self.entries.iter().rev() {
                writeln!(f, "{entry}")?;
            }
            f.flush()?;
            Ok(())
        })();
        if ok.is_ok() {
            let _ = fs::rename(&tmp, &path);
        } else {
            let _ = fs::remove_file(&tmp);
        }
    }

    /// Move up (older). Returns the history entry to show.
    pub(super) fn up(&mut self, current_text: &str) -> Option<&str> {
        let next = match self.cursor {
            None => {
                self.stashed = current_text.to_string();
                0
            }
            Some(i) => i + 1,
        };
        if next < self.entries.len() {
            self.cursor = Some(next);
            Some(&self.entries[next])
        } else {
            self.cursor
                .and_then(|i| self.entries.get(i).map(|s| s.as_str()))
        }
    }

    /// Move down (newer). Returns the history entry, or `None` if not browsing.
    pub(super) fn down(&mut self) -> Option<&str> {
        match self.cursor {
            None => None,
            Some(0) => {
                self.cursor = None;
                Some(&self.stashed)
            }
            Some(i) => {
                let next = i - 1;
                self.cursor = Some(next);
                Some(&self.entries[next])
            }
        }
    }

    pub(super) fn reset_browse(&mut self) {
        self.cursor = None;
        self.stashed.clear();
    }

    /// Returns `(1-based position, total)` when browsing history, or `None`.
    pub(super) fn browse_position(&self) -> Option<(usize, usize)> {
        self.cursor.map(|i| (i + 1, self.entries.len()))
    }
}
