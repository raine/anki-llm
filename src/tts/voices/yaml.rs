//! Emit a pasteable `tts:` YAML block for a selected voice.
//!
//! The block is a scaffold, not a complete drop-in: `target` and
//! `source` are deck-design fields the user must fill in, so we stamp
//! `<field>` placeholders to make that obvious. Provider-specific
//! fields (`region`, `model`) are included when applicable so the
//! user doesn't need to cross-reference the provider docs.

use super::catalog::{ProviderId, VoiceEntry};

pub fn emit_scaffold(entry: &VoiceEntry, region_override: Option<&str>) -> String {
    let mut out = String::from("tts:\n");
    out.push_str("  target: Audio\n");
    out.push_str("  source:\n");
    out.push_str("    field: <field>\n");
    out.push_str(&format!("  provider: {}\n", entry.provider.as_str()));
    match entry.provider {
        ProviderId::Azure | ProviderId::Amazon => {
            let region = region_override.unwrap_or("<region>");
            out.push_str(&format!("  region: {region}\n"));
        }
        _ => {}
    }
    if let Some(model) = &entry.preview_model {
        out.push_str(&format!("  model: {model}\n"));
    }
    out.push_str(&format!("  voice: {}\n", entry.voice_id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(provider: ProviderId, voice: &str) -> VoiceEntry {
        VoiceEntry {
            provider,
            voice_id: voice.into(),
            display_name: voice.into(),
            languages: vec!["ja-JP".into()],
            multilingual: false,
            gender: None,
            preview_model: None,
            tags: vec![],
        }
    }

    #[test]
    fn openai_scaffold_has_no_region() {
        let e = entry(ProviderId::Openai, "alloy");
        let y = emit_scaffold(&e, None);
        assert!(y.contains("provider: openai"));
        assert!(y.contains("voice: alloy"));
        assert!(!y.contains("region"));
    }

    #[test]
    fn azure_scaffold_has_region_placeholder_when_missing() {
        let e = entry(ProviderId::Azure, "ja-JP-NanamiNeural");
        let y = emit_scaffold(&e, None);
        assert!(y.contains("region: <region>"));
    }

    #[test]
    fn azure_scaffold_uses_override_region() {
        let e = entry(ProviderId::Azure, "ja-JP-NanamiNeural");
        let y = emit_scaffold(&e, Some("eastus"));
        assert!(y.contains("region: eastus"));
    }

    #[test]
    fn amazon_scaffold_includes_model_when_set() {
        let mut e = entry(ProviderId::Amazon, "Takumi");
        e.preview_model = Some("neural".into());
        let y = emit_scaffold(&e, Some("us-east-1"));
        assert!(y.contains("provider: amazon"));
        assert!(y.contains("region: us-east-1"));
        assert!(y.contains("model: neural"));
        assert!(y.contains("voice: Takumi"));
    }
}
