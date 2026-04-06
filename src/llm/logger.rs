use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Logger for raw LLM prompts and responses. Thread-safe via internal mutex.
///
/// Supports two output modes (both can be active simultaneously):
/// - `--log <path>`: append each prompt/response pair to a file
/// - `--very-verbose`: print each prompt/response pair to stderr
pub struct LlmLogger {
    file: Option<Mutex<File>>,
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
        Ok(Self { file, very_verbose })
    }

    pub fn is_active(&self) -> bool {
        self.file.is_some() || self.very_verbose
    }

    /// Log a prompt/response pair. No-op if neither file nor very-verbose is set.
    pub fn log(&self, prompt: &str, response: &str) {
        if !self.is_active() {
            return;
        }
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let entry = format!("[{ts}]\n--- PROMPT ---\n{prompt}\n--- RESPONSE ---\n{response}\n\n");
        if let Some(ref file) = self.file
            && let Ok(mut f) = file.lock()
        {
            let _ = f.write_all(entry.as_bytes());
        }
        if self.very_verbose {
            eprint!("{entry}");
        }
    }
}
