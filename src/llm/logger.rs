use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Logger for raw LLM prompts and responses. Thread-safe via internal mutex.
///
/// Supports three output modes (all can be active simultaneously):
/// - Automatic: always writes to `~/.local/state/anki-llm/logs/<timestamp>.log`
/// - `--log <path>`: append each prompt/response pair to a user-specified file
/// - `--very-verbose`: print each prompt/response pair to stderr
pub struct LlmLogger {
    /// User-specified log file (--log flag).
    file: Option<Mutex<File>>,
    /// Automatic per-session log in state dir.
    auto_file: Option<Mutex<File>>,
    pub very_verbose: bool,
}

impl LlmLogger {
    pub fn new(log_path: Option<&Path>, very_verbose: bool) -> anyhow::Result<Self> {
        let file = log_path
            .map(|p| {
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(p)
                    .map(Mutex::new)
                    .map_err(|e| anyhow::anyhow!("failed to open log file {}: {}", p.display(), e))
            })
            .transpose()?;

        let auto_file = open_auto_log().ok().map(Mutex::new);

        Ok(Self {
            file,
            auto_file,
            very_verbose,
        })
    }

    pub fn is_active(&self) -> bool {
        self.file.is_some() || self.auto_file.is_some() || self.very_verbose
    }

    /// Log a prompt/response pair. No-op if no output target is available.
    pub fn log(&self, prompt: &str, response: &str) {
        if !self.is_active() {
            return;
        }
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = format!("[{ts}]\n--- PROMPT ---\n{prompt}\n--- RESPONSE ---\n{response}\n\n");
        self.write(&entry);
    }

    /// Log an error for a prompt that failed. No-op if no output target is available.
    pub fn log_error(&self, prompt: &str, error: &str) {
        if !self.is_active() {
            return;
        }
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = format!("[{ts}]\n--- PROMPT ---\n{prompt}\n--- ERROR ---\n{error}\n\n");
        self.write(&entry);
    }

    fn write(&self, entry: &str) {
        for file_mutex in [&self.file, &self.auto_file].into_iter().flatten() {
            if let Ok(mut f) = file_mutex.lock() {
                let _ = f.write_all(entry.as_bytes());
            }
        }

        if self.very_verbose {
            eprint!("{entry}");
        }
    }
}

/// Open a per-session auto-log file at `~/.local/state/anki-llm/logs/<timestamp>.log`.
fn open_auto_log() -> anyhow::Result<File> {
    let home = home::home_dir().ok_or_else(|| anyhow::anyhow!("no home dir"))?;
    let logs_dir = home
        .join(".local")
        .join("state")
        .join("anki-llm")
        .join("logs");
    std::fs::create_dir_all(&logs_dir)?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = logs_dir.join(format!("{ts}.log"));

    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    Ok(file)
}
