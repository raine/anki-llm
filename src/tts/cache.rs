use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::provider::SynthesisRequest;

/// Cache schema version. Bump this when the hash inputs change in a way
/// that should invalidate existing cached audio. v2 switched from hashing
/// the normalized raw text to hashing the exact prepared payload (plain
/// text or SSML) that the provider POSTs.
pub const CACHE_SCHEMA_VERSION: u32 = 2;

/// Monotonic counter used to disambiguate concurrent temp filenames so
/// two workers synthesizing the same request can't clobber each other's
/// in-flight write. Combined with the process ID this is unique within a
/// single anki-llm invocation.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct TtsCache {
    dir: PathBuf,
}

impl TtsCache {
    pub fn new(dir: PathBuf) -> std::io::Result<Self> {
        fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Default on-disk cache location: `~/.cache/anki-llm/tts/`.
    pub fn default_dir() -> Option<PathBuf> {
        home::home_dir().map(|h| h.join(".cache").join("anki-llm").join("tts"))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Canonical cache key: SHA-256 over a stable serialization of every
    /// input that affects the audio output (including the cache schema
    /// version). Same inputs always produce the same key.
    pub fn key(req: &SynthesisRequest) -> String {
        let mut h = Sha256::new();
        h.update(format!("v{}\n", CACHE_SCHEMA_VERSION).as_bytes());
        h.update(format!("provider={}\n", req.provider_id).as_bytes());
        h.update(format!("text_format={}\n", req.text_format.tag()).as_bytes());
        h.update(format!("endpoint={}\n", req.endpoint.as_deref().unwrap_or("")).as_bytes());
        h.update(format!("voice={}\n", req.voice).as_bytes());
        h.update(format!("format={}\n", req.format.ext()).as_bytes());
        h.update(format!("model={}\n", req.model.as_deref().unwrap_or("")).as_bytes());
        h.update(format!("speed={}\n", req.speed.unwrap_or(1.0)).as_bytes());
        h.update("payload=".as_bytes());
        h.update(req.payload.as_bytes());
        h.update(b"\n");
        hex(&h.finalize())
    }

    pub fn filename(req: &SynthesisRequest) -> String {
        format!("anki-llm-tts-{}.{}", Self::key(req), req.format.ext())
    }

    pub fn path_for(&self, req: &SynthesisRequest) -> PathBuf {
        self.dir.join(Self::filename(req))
    }

    /// Return cached audio bytes if present and non-empty. Zero-byte files
    /// are treated as misses (they indicate a partially-written cache
    /// entry).
    pub fn load(&self, req: &SynthesisRequest) -> Option<Vec<u8>> {
        let path = self.path_for(req);
        match fs::metadata(&path) {
            Ok(m) if m.len() > 0 => fs::read(&path).ok(),
            _ => None,
        }
    }

    /// Write audio bytes to the cache atomically (write to a per-call
    /// unique temp file, then rename over the final path) so concurrent
    /// workers synthesizing the same request can't clobber each other, and
    /// a crash mid-write cannot leave a truncated entry that `load` would
    /// return.
    pub fn store(&self, req: &SynthesisRequest, bytes: &[u8]) -> std::io::Result<PathBuf> {
        let path = self.path_for(req);
        let seq = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        let tmp_name = format!(
            "{}.{}.{}.{}.tmp",
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("anki-llm-tts"),
            pid,
            seq,
            req.format.ext()
        );
        let tmp = self.dir.join(tmp_name);
        fs::write(&tmp, bytes)?;
        // Rename is atomic on the same filesystem. If two workers race,
        // whichever rename happens second just replaces the first file's
        // contents — both copies are bit-identical for the same request.
        fs::rename(&tmp, &path)?;
        Ok(path)
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tts::provider::{AudioFormat, TextFormat};

    fn req(payload: &str) -> SynthesisRequest {
        SynthesisRequest {
            payload: payload.to_string(),
            provider_id: "openai".into(),
            text_format: TextFormat::PlainText,
            voice: "alloy".into(),
            format: AudioFormat::Mp3,
            model: Some("gpt-4o-mini-tts".into()),
            speed: None,
            endpoint: None,
        }
    }

    #[test]
    fn same_request_same_key() {
        let a = TtsCache::key(&req("hello world"));
        let b = TtsCache::key(&req("hello world"));
        assert_eq!(a, b);
    }

    #[test]
    fn different_payload_different_key() {
        assert_ne!(TtsCache::key(&req("hello")), TtsCache::key(&req("world")));
    }

    #[test]
    fn different_endpoint_different_key() {
        let mut r1 = req("hi");
        let mut r2 = req("hi");
        r1.endpoint = Some("https://api.openai.com/v1".into());
        r2.endpoint = Some("https://alt.example.com/v1".into());
        assert_ne!(TtsCache::key(&r1), TtsCache::key(&r2));
    }

    #[test]
    fn different_voice_different_key() {
        let mut r1 = req("hi");
        let mut r2 = req("hi");
        r2.voice = "nova".into();
        assert_ne!(TtsCache::key(&r1), TtsCache::key(&r2));
        r1.voice = "alloy".into();
        assert_eq!(TtsCache::key(&r1), TtsCache::key(&req("hi")));
    }

    #[test]
    fn different_provider_different_key() {
        let mut r1 = req("hi");
        let mut r2 = req("hi");
        r2.provider_id = "azure".into();
        r2.text_format = TextFormat::Ssml;
        assert_ne!(TtsCache::key(&r1), TtsCache::key(&r2));
        r1.provider_id = "openai".into();
    }

    #[test]
    fn different_text_format_different_key() {
        let mut r1 = req("hi");
        let mut r2 = req("hi");
        r2.text_format = TextFormat::Ssml;
        assert_ne!(TtsCache::key(&r1), TtsCache::key(&r2));
    }

    #[test]
    fn filename_has_format_extension() {
        let f = TtsCache::filename(&req("hi"));
        assert!(f.ends_with(".mp3"));
        assert!(f.starts_with("anki-llm-tts-"));
    }

    #[test]
    fn store_and_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = TtsCache::new(tmp.path().to_path_buf()).unwrap();
        let r = req("hello");
        assert!(cache.load(&r).is_none());
        cache.store(&r, b"audiodata").unwrap();
        assert_eq!(cache.load(&r).as_deref(), Some(b"audiodata" as &[u8]));
    }

    #[test]
    fn zero_byte_file_is_miss() {
        let tmp = tempfile::tempdir().unwrap();
        let cache = TtsCache::new(tmp.path().to_path_buf()).unwrap();
        let r = req("hi");
        std::fs::write(cache.path_for(&r), b"").unwrap();
        assert!(cache.load(&r).is_none());
    }
}
