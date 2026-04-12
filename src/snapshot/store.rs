use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

/// A single note's before/after field state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NoteRevision {
    pub note_id: i64,
    /// All field values before processing.
    pub before_fields: IndexMap<String, String>,
    /// Only fields that actually changed (sparse).
    pub after_fields: IndexMap<String, String>,
}

/// Metadata + note revisions for a single process-deck run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub run_id: String,
    pub timestamp: String,
    /// Deck name when the run targeted a specific deck. `None` for query-mode
    /// runs (see [`Snapshot::query`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deck: Option<String>,
    /// Raw Anki search query when the run was launched with `--query`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    pub model: String,
    pub note_count: usize,
    #[serde(default)]
    pub rolled_back: bool,
    pub notes: Vec<NoteRevision>,
}

impl Snapshot {
    /// Human-readable label identifying the run's source. Prefers the deck
    /// name; falls back to the raw query, then to a placeholder.
    pub fn source_display(&self) -> String {
        if let Some(deck) = self.deck.as_deref().filter(|s| !s.is_empty()) {
            deck.to_string()
        } else if let Some(q) = self.query.as_deref().filter(|s| !s.is_empty()) {
            format!("query: {q}")
        } else {
            "(unknown)".to_string()
        }
    }
}

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let status = if self.rolled_back {
            "rolled back"
        } else {
            "ok"
        };
        write!(
            f,
            "{:<22} {:<24} {:<18} {:>5}  {}",
            self.run_id,
            self.source_display(),
            self.model,
            self.note_count,
            status
        )
    }
}

/// Return the snapshots directory path.
pub fn snapshots_dir() -> Result<PathBuf> {
    let home = home::home_dir().context("could not determine home directory")?;
    Ok(home
        .join(".local")
        .join("state")
        .join("anki-llm")
        .join("snapshots"))
}

/// Generate a sortable run ID from current UTC time: 20260411T153000_123Z
/// Includes milliseconds to avoid collisions from rapid successive runs.
pub fn generate_run_id() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();

    // Manual UTC breakdown (no chrono needed)
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since epoch to y/m/d
    let (year, month, day) = days_to_ymd(days);

    format!("{year:04}{month:02}{day:02}T{hours:02}{minutes:02}{seconds:02}_{millis:03}Z")
}

/// Generate a human-readable UTC timestamp.
pub fn generate_timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm from http://howardhinnant.github.io/date_algorithms.html
    let z = days + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

/// Save a snapshot to disk atomically.
pub fn save_snapshot(snapshot: &Snapshot) -> Result<PathBuf> {
    let dir = snapshots_dir()?;
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create snapshots dir: {}", dir.display()))?;
    let path = dir.join(format!("{}.json", snapshot.run_id));
    let json = serde_json::to_string_pretty(snapshot).context("failed to serialize snapshot")?;
    atomic_write(&path, &json)?;
    Ok(path)
}

/// Load a snapshot by run ID.
pub fn load_snapshot(run_id: &str) -> Result<Snapshot> {
    validate_run_id(run_id)?;
    let path = snapshots_dir()?.join(format!("{run_id}.json"));
    let content =
        fs::read_to_string(&path).with_context(|| format!("snapshot not found: {run_id}"))?;
    let snapshot: Snapshot = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse snapshot: {}", path.display()))?;
    Ok(snapshot)
}

/// Validate that a run ID contains only safe characters (alphanumeric, T, Z, _).
fn validate_run_id(run_id: &str) -> Result<()> {
    if run_id.is_empty()
        || !run_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == 'T' || c == 'Z' || c == '_')
    {
        anyhow::bail!("invalid run ID: {run_id}");
    }
    Ok(())
}

/// List all snapshots sorted by run_id descending (most recent first).
pub fn list_snapshots() -> Result<Vec<Snapshot>> {
    let dir = snapshots_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let content = fs::read_to_string(&path)?;
            if let Ok(snap) = serde_json::from_str::<Snapshot>(&content) {
                snapshots.push(snap);
            }
        }
    }
    snapshots.sort_by(|a, b| b.run_id.cmp(&a.run_id));
    Ok(snapshots)
}

fn atomic_write(path: &Path, content: &str) -> Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = NamedTempFile::new_in(dir)?;
    tmp.write_all(content.as_bytes())?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_id_is_sortable() {
        let id1 = generate_run_id();
        // Sleep briefly to ensure different millisecond
        std::thread::sleep(std::time::Duration::from_millis(5));
        let id2 = generate_run_id();
        assert!(id2 > id1, "run IDs should be lexicographically sortable");
    }

    #[test]
    fn days_to_ymd_epoch() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2026-04-11 is day 20554 since epoch
        assert_eq!(days_to_ymd(20554), (2026, 4, 11));
    }

    #[test]
    fn snapshot_round_trip() {
        let snap = Snapshot {
            run_id: "20260411T120000Z".into(),
            timestamp: "2026-04-11T12:00:00Z".into(),
            deck: Some("Test".into()),
            query: None,
            model: "gpt-5-mini".into(),
            note_count: 1,
            rolled_back: false,
            notes: vec![NoteRevision {
                note_id: 123,
                before_fields: IndexMap::from([("Front".into(), "old".into())]),
                after_fields: IndexMap::from([("Front".into(), "new".into())]),
            }],
        };
        let json = serde_json::to_string(&snap).unwrap();
        let loaded: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.run_id, snap.run_id);
        assert_eq!(loaded.notes.len(), 1);
        assert_eq!(loaded.notes[0].note_id, 123);
    }

    #[test]
    fn legacy_snapshot_with_string_deck_loads() {
        // Older snapshots stored `deck` as a plain string. Ensure they still
        // deserialize into the current Option<String> shape.
        let legacy = r#"{
            "run_id": "20260411T120000Z",
            "timestamp": "2026-04-11T12:00:00Z",
            "deck": "Old Deck",
            "model": "gpt-5-mini",
            "note_count": 0,
            "notes": []
        }"#;
        let snap: Snapshot = serde_json::from_str(legacy).unwrap();
        assert_eq!(snap.deck.as_deref(), Some("Old Deck"));
        assert_eq!(snap.query, None);
        assert_eq!(snap.source_display(), "Old Deck");
    }

    #[test]
    fn query_mode_snapshot_source_display() {
        let snap = Snapshot {
            run_id: "20260411T120000Z".into(),
            timestamp: "2026-04-11T12:00:00Z".into(),
            deck: None,
            query: Some(r#"deck:"Oma dekki" tag:foo"#.into()),
            model: "gpt-5-mini".into(),
            note_count: 0,
            rolled_back: false,
            notes: vec![],
        };
        assert_eq!(snap.source_display(), r#"query: deck:"Oma dekki" tag:foo"#);
    }

    #[test]
    fn save_and_load_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        // Override HOME so snapshots_dir() points to temp
        unsafe { std::env::set_var("HOME", tmp.path()) };

        let snap = Snapshot {
            run_id: "20260411T120000Z".into(),
            timestamp: "2026-04-11T12:00:00Z".into(),
            deck: Some("Test".into()),
            query: None,
            model: "gpt-5-mini".into(),
            note_count: 0,
            rolled_back: false,
            notes: vec![],
        };
        save_snapshot(&snap).unwrap();
        let loaded = load_snapshot("20260411T120000Z").unwrap();
        assert_eq!(loaded.deck.as_deref(), Some("Test"));

        unsafe { std::env::remove_var("HOME") };
    }
}
