//! Normalized voice catalog loaded from a committed JSON snapshot.
//!
//! The snapshot at `src/tts/voices/snapshot.json` is shipped verbatim in
//! the binary via `include_str!`. A future phase can add live-refresh
//! against each provider's list API behind the same `VoiceEntry` type;
//! for now we rely on the checked-in snapshot.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Openai,
    Azure,
    Google,
    Amazon,
}

impl ProviderId {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderId::Openai => "openai",
            ProviderId::Azure => "azure",
            ProviderId::Google => "google",
            ProviderId::Amazon => "amazon",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "openai" => Some(Self::Openai),
            "azure" => Some(Self::Azure),
            "google" => Some(Self::Google),
            "amazon" => Some(Self::Amazon),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct VoiceEntry {
    pub provider: ProviderId,
    /// Exact string to drop into `tts.voice` in a frontmatter block.
    pub voice_id: String,
    /// Human-readable display name (e.g. `"Nanami (Neural)"`).
    pub display_name: String,
    /// BCP-47 codes the voice natively speaks. Empty + `multilingual`
    /// true means the voice is treated as language-agnostic (OpenAI).
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub multilingual: bool,
    #[serde(default)]
    pub gender: Option<String>,
    /// Required `tts.model` for this voice, if any. Used for Polly
    /// `Engine` (`standard` / `neural` / `generative` / `long-form`).
    #[serde(default)]
    pub preview_model: Option<String>,
    /// Free-form tags from the snapshot converter (`chirp3`, `wavenet`,
    /// `dragon-hd`, `neural`, ...). Used for omni-search matching.
    #[serde(default)]
    pub tags: Vec<String>,
}

impl VoiceEntry {
    /// Primary language used for sample-text selection and sort order.
    pub fn primary_language(&self) -> &str {
        self.languages.first().map(String::as_str).unwrap_or("en")
    }

    /// Lowercase concatenation of every field used for omni-search.
    /// Cached form of this string lives in the filter index.
    pub fn searchable_text(&self) -> String {
        let mut s = String::new();
        s.push_str(self.provider.as_str());
        s.push(' ');
        s.push_str(&self.voice_id.to_ascii_lowercase());
        s.push(' ');
        s.push_str(&self.display_name.to_ascii_lowercase());
        for lang in &self.languages {
            s.push(' ');
            s.push_str(&lang.to_ascii_lowercase());
        }
        if let Some(g) = &self.gender {
            s.push(' ');
            s.push_str(g);
        }
        if let Some(m) = &self.preview_model {
            s.push(' ');
            s.push_str(m);
        }
        for t in &self.tags {
            s.push(' ');
            s.push_str(t);
        }
        if self.multilingual {
            s.push_str(" multilingual");
        }
        s
    }
}

/// Load the committed snapshot. Panics on malformed JSON — that's a
/// build-time problem, not a runtime one.
pub fn load_snapshot() -> Vec<VoiceEntry> {
    const SNAPSHOT: &str = include_str!("snapshot.json");
    serde_json::from_str(SNAPSHOT).expect("tts/voices/snapshot.json is malformed")
}

/// Filter result: indices into the input slice, in ranked order.
///
/// Ranking rules:
/// - If `lang_filter` is set, only voices whose primary or any listed
///   language starts with it (case-insensitive) are kept. Multilingual
///   voices are kept regardless.
/// - If `provider_filter` is set, non-matching voices are dropped.
/// - Omni-query is split on whitespace; each token must appear as a
///   case-insensitive substring in the voice's searchable text. Empty
///   query keeps everything.
pub fn filter(
    entries: &[VoiceEntry],
    query: &str,
    lang_filter: Option<&str>,
    provider_filter: Option<ProviderId>,
) -> Vec<usize> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect();
    let lang_lc = lang_filter.map(|s| s.to_ascii_lowercase());

    let mut out: Vec<usize> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if let Some(p) = provider_filter
            && entry.provider != p
        {
            continue;
        }
        if let Some(lang) = &lang_lc {
            let matches_lang = entry.multilingual
                || entry.languages.iter().any(|l| {
                    let lc = l.to_ascii_lowercase();
                    lc == *lang || lc.starts_with(&format!("{lang}-"))
                });
            if !matches_lang {
                continue;
            }
        }
        if !tokens.is_empty() {
            let hay = entry.searchable_text();
            if !tokens.iter().all(|t| hay.contains(t)) {
                continue;
            }
        }
        out.push(i);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load() -> Vec<VoiceEntry> {
        load_snapshot()
    }

    #[test]
    fn snapshot_parses_and_has_all_providers() {
        let entries = load();
        assert!(entries.len() > 2000, "expected a full snapshot");
        let mut seen = std::collections::HashSet::new();
        for e in &entries {
            seen.insert(e.provider);
        }
        assert!(seen.contains(&ProviderId::Openai));
        assert!(seen.contains(&ProviderId::Azure));
        assert!(seen.contains(&ProviderId::Google));
        assert!(seen.contains(&ProviderId::Amazon));
    }

    #[test]
    fn filter_by_language_narrows_to_ja() {
        let entries = load();
        let ids = filter(&entries, "", Some("ja"), None);
        assert!(!ids.is_empty());
        for i in &ids {
            let e = &entries[*i];
            assert!(
                e.multilingual || e.languages.iter().any(|l| l.starts_with("ja")),
                "non-ja voice slipped through: {e:?}"
            );
        }
    }

    #[test]
    fn filter_by_provider_narrows_to_azure() {
        let entries = load();
        let ids = filter(&entries, "", None, Some(ProviderId::Azure));
        assert!(!ids.is_empty());
        assert!(
            ids.iter()
                .all(|i| entries[*i].provider == ProviderId::Azure)
        );
    }

    #[test]
    fn filter_query_requires_all_tokens() {
        let entries = load();
        let ids = filter(
            &entries,
            "nanami neural",
            Some("ja"),
            Some(ProviderId::Azure),
        );
        assert!(
            ids.iter().any(|i| entries[*i].voice_id.contains("Nanami")),
            "expected NanamiNeural in filtered results"
        );
    }

    #[test]
    fn providerid_roundtrip() {
        for id in [
            ProviderId::Openai,
            ProviderId::Azure,
            ProviderId::Google,
            ProviderId::Amazon,
        ] {
            assert_eq!(ProviderId::parse(id.as_str()), Some(id));
        }
        assert_eq!(ProviderId::parse("unknown"), None);
    }
}
