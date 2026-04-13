//! Normalized voice catalog loaded from a committed JSON snapshot.
//!
//! The snapshot at `src/tts/voices/snapshot.json` is shipped verbatim in
//! the binary via `include_str!`. A future phase can add live-refresh
//! against each provider's list API behind the same `VoiceEntry` type;
//! for now we rely on the checked-in snapshot.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
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

/// Structured filter state driven by the TUI chip row + modal pickers.
///
/// `text` is a free-form substring match against voice id + display
/// name only. Structured facets (`provider`, `language`, `gender`,
/// `engine`, `tags`) own everything else.
#[derive(Debug, Clone, Default)]
pub struct VoiceFilters {
    pub provider: Option<ProviderId>,
    /// BCP-47 prefix (e.g. `ja`, `ja-JP`). Matches voices whose primary
    /// or any listed language equals or starts with the prefix.
    /// Multilingual voices (OpenAI) are always kept.
    pub language: Option<String>,
    pub gender: Option<String>,
    /// Polly `Engine` (`standard`, `neural`, `generative`, `long-form`).
    /// Other providers carry an empty `preview_model` and always pass.
    pub engine: Option<String>,
    /// Required tags — ALL must be present on the entry.
    pub tags: Vec<String>,
    /// Free-text substring over voice_id + display_name (case-insensitive).
    pub text: String,
}

impl VoiceFilters {
    pub fn is_empty(&self) -> bool {
        self.provider.is_none()
            && self.language.is_none()
            && self.gender.is_none()
            && self.engine.is_none()
            && self.tags.is_empty()
            && self.text.is_empty()
    }

    pub fn active_count(&self) -> usize {
        let mut n = 0;
        if self.provider.is_some() {
            n += 1;
        }
        if self.language.is_some() {
            n += 1;
        }
        if self.gender.is_some() {
            n += 1;
        }
        if self.engine.is_some() {
            n += 1;
        }
        n += self.tags.len();
        if !self.text.is_empty() {
            n += 1;
        }
        n
    }
}

/// Apply `filters` to `entries` and return the retained indices in
/// original order.
pub fn filter(entries: &[VoiceEntry], filters: &VoiceFilters) -> Vec<usize> {
    let lang_lc = filters.language.as_deref().map(str::to_ascii_lowercase);
    let gender_lc = filters.gender.as_deref().map(str::to_ascii_lowercase);
    let engine_lc = filters.engine.as_deref().map(str::to_ascii_lowercase);
    let text_lc = filters.text.to_ascii_lowercase();
    let text_tokens: Vec<&str> = if text_lc.is_empty() {
        Vec::new()
    } else {
        text_lc.split_whitespace().collect()
    };

    let mut out: Vec<usize> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if let Some(p) = filters.provider
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
        if let Some(gender) = &gender_lc {
            match &entry.gender {
                Some(g) if g.to_ascii_lowercase() == *gender => {}
                _ => continue,
            }
        }
        if let Some(engine) = &engine_lc {
            match &entry.preview_model {
                Some(m) if m.to_ascii_lowercase() == *engine => {}
                _ => continue,
            }
        }
        if !filters.tags.is_empty() {
            let have: std::collections::HashSet<&str> =
                entry.tags.iter().map(String::as_str).collect();
            let all_present = filters.tags.iter().all(|t| have.contains(t.as_str()));
            if !all_present {
                continue;
            }
        }
        if !text_tokens.is_empty() {
            let id_lc = entry.voice_id.to_ascii_lowercase();
            let name_lc = entry.display_name.to_ascii_lowercase();
            let all_match = text_tokens
                .iter()
                .all(|t| id_lc.contains(t) || name_lc.contains(t));
            if !all_match {
                continue;
            }
        }
        out.push(i);
    }
    out
}

/// Precomputed facet options for the modal pickers: unique values
/// and the count of voices having each value, sorted for display.
/// Built once at startup from the catalog snapshot.
#[derive(Debug, Clone, Default)]
pub struct FacetCatalog {
    pub providers: Vec<(ProviderId, usize)>,
    pub languages: Vec<(String, usize)>,
    pub genders: Vec<(String, usize)>,
    pub engines: Vec<(String, usize)>,
    pub tags: Vec<(String, usize)>,
}

pub fn build_facets(entries: &[VoiceEntry]) -> FacetCatalog {
    use std::collections::BTreeMap;
    let mut providers: BTreeMap<ProviderId, usize> = BTreeMap::new();
    let mut languages: BTreeMap<String, usize> = BTreeMap::new();
    let mut genders: BTreeMap<String, usize> = BTreeMap::new();
    let mut engines: BTreeMap<String, usize> = BTreeMap::new();
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();

    for entry in entries {
        *providers.entry(entry.provider).or_default() += 1;
        for lang in &entry.languages {
            *languages.entry(lang.clone()).or_default() += 1;
        }
        if let Some(g) = &entry.gender {
            *genders.entry(g.clone()).or_default() += 1;
        }
        if let Some(m) = &entry.preview_model {
            *engines.entry(m.clone()).or_default() += 1;
        }
        for t in &entry.tags {
            *tags.entry(t.clone()).or_default() += 1;
        }
    }

    FacetCatalog {
        providers: providers.into_iter().collect(),
        languages: languages.into_iter().collect(),
        genders: genders.into_iter().collect(),
        engines: engines.into_iter().collect(),
        tags: tags.into_iter().collect(),
    }
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
        let ids = filter(
            &entries,
            &VoiceFilters {
                language: Some("ja".into()),
                ..VoiceFilters::default()
            },
        );
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
        let ids = filter(
            &entries,
            &VoiceFilters {
                provider: Some(ProviderId::Azure),
                ..VoiceFilters::default()
            },
        );
        assert!(!ids.is_empty());
        assert!(
            ids.iter()
                .all(|i| entries[*i].provider == ProviderId::Azure)
        );
    }

    #[test]
    fn text_search_matches_voice_id_and_name() {
        let entries = load();
        let ids = filter(
            &entries,
            &VoiceFilters {
                provider: Some(ProviderId::Azure),
                language: Some("ja".into()),
                text: "nanami".into(),
                ..VoiceFilters::default()
            },
        );
        assert!(
            ids.iter().any(|i| entries[*i].voice_id.contains("Nanami")),
            "expected NanamiNeural in filtered results"
        );
    }

    #[test]
    fn build_facets_collects_sorted_values() {
        let entries = load();
        let facets = build_facets(&entries);
        assert_eq!(facets.providers.len(), 4);
        assert!(
            facets
                .providers
                .iter()
                .any(|(p, _)| *p == ProviderId::Amazon)
        );
        assert!(facets.languages.iter().any(|(lang, _)| lang == "ja-JP"));
        assert!(facets.tags.iter().any(|(tag, _)| tag == "neural"));
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
