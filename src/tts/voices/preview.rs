//! Background synthesis worker: serializes `TtsService::ensure_cached`
//! calls so the ratatui main loop never blocks on a network request.
//!
//! The app layer assigns each request a monotonically-increasing id.
//! When the user mashes Space the app sends a new request and drops
//! results tagged with older ids. The worker itself is oblivious — it
//! just processes requests serially as they arrive.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use indexmap::IndexMap;

use crate::tts::cache::TtsCache;
use crate::tts::provider::{AudioFormat, build as build_provider};
use crate::tts::service::{TtsService, TtsServiceConfig};
use crate::tts::template::TemplateSource;

use super::catalog::VoiceEntry;
use super::credentials::ProviderPreviewState;
use super::sample::{pangram_for, sample_lang_for};

/// Opaque monotonic id. App-assigned.
pub type RequestId = u64;

pub struct PreviewRequest {
    pub id: RequestId,
    pub entry: VoiceEntry,
    pub state: ProviderPreviewState,
    pub cache: Arc<TtsCache>,
}

pub enum PreviewResult {
    Ok { id: RequestId, path: PathBuf },
    Err { id: RequestId, message: String },
}

pub struct PreviewHandle {
    tx: Sender<Option<PreviewRequest>>,
    pub rx: Receiver<PreviewResult>,
}

impl PreviewHandle {
    pub fn submit(&self, req: PreviewRequest) {
        let _ = self.tx.send(Some(req));
    }

    /// Best-effort shutdown: signals the worker to stop reading. The
    /// worker thread exits on its own once the channel is drained.
    pub fn shutdown(&self) {
        let _ = self.tx.send(None);
    }
}

pub fn spawn_worker() -> PreviewHandle {
    let (req_tx, req_rx) = mpsc::channel::<Option<PreviewRequest>>();
    let (res_tx, res_rx) = mpsc::channel::<PreviewResult>();
    thread::spawn(move || {
        while let Ok(Some(req)) = req_rx.recv() {
            let id = req.id;
            match synthesize_one(req) {
                Ok(path) => {
                    let _ = res_tx.send(PreviewResult::Ok { id, path });
                }
                Err(msg) => {
                    let _ = res_tx.send(PreviewResult::Err { id, message: msg });
                }
            }
        }
    });
    PreviewHandle {
        tx: req_tx,
        rx: res_rx,
    }
}

fn synthesize_one(req: PreviewRequest) -> Result<PathBuf, String> {
    let PreviewRequest {
        entry,
        state,
        cache,
        ..
    } = req;

    let (selection, endpoint) = match state {
        ProviderPreviewState::Ready {
            selection,
            endpoint_identity,
        } => (selection, endpoint_identity),
        ProviderPreviewState::Unavailable { reason } => return Err(reason),
    };

    let provider = build_provider(selection);
    let lang = sample_lang_for(&entry);
    let sample_text = pangram_for(lang);

    // Inline TemplateSource with a label that identifies the preview
    // context in cache-diagnostic output. Contents have no `{placeholder}`
    // references, so expansion against an empty row returns the pangram
    // verbatim.
    let source = Arc::new(TemplateSource::inline(
        "tts-voices-sample".into(),
        sample_text.into(),
    ));

    let service = TtsService::new(TtsServiceConfig {
        provider,
        cache,
        source,
        target_field: "_preview".into(),
        voice: entry.voice_id.clone(),
        model: entry.preview_model.clone(),
        format: AudioFormat::Mp3,
        speed: None,
        endpoint,
    });

    let empty_row: IndexMap<String, serde_json::Value> = IndexMap::new();
    let prepared = service
        .prepare_from_row(&empty_row)
        .map_err(|e| format!("prepare failed: {e}"))?;
    service
        .ensure_cached(&prepared)
        .map_err(|e| format!("synthesis failed: {e}"))?;
    Ok(prepared.cache_path)
}
