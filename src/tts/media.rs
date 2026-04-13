use std::collections::HashMap;
use std::sync::{Condvar, Mutex};

use crate::anki::client::AnkiClient;
use crate::tts::error::TtsError;

/// State of a cached-filename upload slot.
#[derive(Clone)]
enum UploadState {
    /// Another worker is currently uploading this filename. Followers wait
    /// on the condvar instead of racing the network call.
    InFlight,
    /// Upload finished — remember the filename AnkiConnect actually stored
    /// (it may differ from the requested name on collision) so subsequent
    /// callers can return it without another round-trip.
    Done(String),
}

/// Thin wrapper around `AnkiClient::store_media_file` that deduplicates
/// uploads of the same cached filename within a single run. Thread-safe:
/// at most one upload is in flight per requested filename, and followers
/// block on a condvar until the first one completes.
pub struct AnkiMediaStore {
    anki: AnkiClient,
    inner: Mutex<HashMap<String, UploadState>>,
    cvar: Condvar,
}

impl AnkiMediaStore {
    pub fn new(anki: AnkiClient) -> Self {
        Self {
            anki,
            inner: Mutex::new(HashMap::new()),
            cvar: Condvar::new(),
        }
    }

    /// Upload `bytes` as `filename` unless another worker has already
    /// uploaded (or is currently uploading) a file with that filename
    /// during this run. Returns the filename AnkiConnect reports it stored
    /// under — which may differ from `filename` on collision.
    pub fn ensure_uploaded(&self, filename: &str, bytes: &[u8]) -> Result<String, TtsError> {
        // Fast path: claim the slot or wait for an in-flight peer.
        let mut guard = self.inner.lock().unwrap();
        loop {
            match guard.get(filename) {
                Some(UploadState::Done(stored)) => return Ok(stored.clone()),
                Some(UploadState::InFlight) => {
                    guard = self.cvar.wait(guard).unwrap();
                    continue;
                }
                None => {
                    guard.insert(filename.to_string(), UploadState::InFlight);
                    break;
                }
            }
        }
        drop(guard);

        // We hold the upload slot — do the network call outside the mutex.
        let result = self
            .anki
            .store_media_file(filename, bytes)
            .map_err(|e| TtsError::Transient(format!("storeMediaFile failed: {e}")));

        let mut guard = self.inner.lock().unwrap();
        match result {
            Ok(stored) => {
                guard.insert(filename.to_string(), UploadState::Done(stored.clone()));
                self.cvar.notify_all();
                Ok(stored)
            }
            Err(e) => {
                // Remove the InFlight marker so another worker can retry.
                guard.remove(filename);
                self.cvar.notify_all();
                Err(e)
            }
        }
    }
}

/// Format an Anki sound reference: `[sound:filename.mp3]`.
pub fn format_sound_tag(filename: &str) -> String {
    format!("[sound:{filename}]")
}

/// Does the given raw field value already contain at least one Anki sound
/// tag? Used to skip rows that already have audio unless the user passed
/// `--force`.
pub fn field_has_sound_tag(value: &str) -> bool {
    value.contains("[sound:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_sound_tag_wraps() {
        assert_eq!(format_sound_tag("foo.mp3"), "[sound:foo.mp3]");
    }

    #[test]
    fn detects_sound_tag() {
        assert!(field_has_sound_tag("hello [sound:a.mp3]"));
        assert!(!field_has_sound_tag("hello"));
        assert!(!field_has_sound_tag(""));
    }
}
