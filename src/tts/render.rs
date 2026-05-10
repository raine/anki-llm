//! Provider-specific rendering of the semantic `Utterance` IR into the
//! exact string each TTS backend wants to receive.
//!
//! Rendering lives outside the provider trait so the caching layer can hash
//! the already-prepared payload. The provider sees the exact bytes it will
//! POST, and the cache key never has to re-derive them.

use super::ir::{Span, Utterance};

/// Render an utterance to plain text for providers that don't understand
/// SSML or ruby markup (OpenAI TTS, etc.).
///
/// Pronunciation spans emit only the reading — the kanji surface is
/// dropped — so that a surface like `転がり込[こ]んだ` becomes
/// `転がりこんだ`, which the provider can pronounce without needing
/// Japanese orthographic knowledge.
pub fn render_plain_text(utterance: &Utterance) -> String {
    let mut out = String::new();
    for span in &utterance.spans {
        match span {
            Span::Text(t) => out.push_str(t),
            Span::Pronunciation { reading, .. } => out.push_str(reading),
        }
    }
    out
}

/// Render an utterance to SSML for Azure Neural TTS.
///
/// The body walks the spans, XML-escapes text, and emits
/// `<sub alias="reading">surface</sub>` for pronunciation spans. The body
/// is then wrapped in a full `<speak>`/`<voice>` envelope bound to the
/// supplied voice. Phase 1 empirically confirmed that the
/// `ja-JP-MasaruMultilingualNeural` voice honors `<sub alias>` substitutions
/// and produces the intended kana reading — which is why we can keep the
/// kanji surface in the SSML instead of dropping it.
pub fn render_ssml(utterance: &Utterance, voice: &str) -> String {
    let mut body = String::new();
    for span in &utterance.spans {
        match span {
            Span::Text(t) => push_xml_escaped(&mut body, t),
            Span::Pronunciation { surface, reading } => {
                body.push_str("<sub alias=\"");
                push_xml_attr_escaped(&mut body, reading);
                body.push_str("\">");
                push_xml_escaped(&mut body, surface);
                body.push_str("</sub>");
            }
        }
    }

    format!(
        "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\" \
         xml:lang=\"ja-JP\"><voice name=\"{voice}\">{body}</voice></speak>",
        voice = xml_attr_escape(voice),
    )
}

pub fn render_edge_ssml(utterance: &Utterance, voice: &str, speed: Option<f32>) -> String {
    let text = xml_escape(&render_plain_text(utterance));
    let voice = xml_attr_escape(voice);
    let rate = xml_attr_escape(&edge_rate(speed));
    format!(
        "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\" \
         xmlns:mstts=\"https://www.w3.org/2001/mstts\" xml:lang=\"en-US\"><voice \
         name=\"{voice}\"><prosody pitch=\"medium\" rate=\"{rate}\" \
         volume=\"medium\">{text}</prosody></voice></speak>"
    )
}

pub fn edge_rate(speed: Option<f32>) -> String {
    let Some(speed) = speed else {
        return "default".to_string();
    };
    if (speed - 1.0).abs() < f32::EPSILON {
        return "default".to_string();
    }
    let percent = ((speed - 1.0) * 100.0).round().clamp(-100.0, 100.0) as i32;
    if percent >= 0 {
        format!("+{percent}%")
    } else {
        format!("{percent}%")
    }
}

fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    push_xml_escaped(&mut out, s);
    out
}

fn push_xml_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
}

fn push_xml_attr_escaped(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
}

fn xml_attr_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    push_xml_attr_escaped(&mut out, s);
    out
}

#[cfg(test)]
mod tests {
    use super::super::ir::{Span, Utterance, parse_furigana};
    use super::*;

    fn text(s: &str) -> Span {
        Span::Text(s.to_string())
    }
    fn pron(surface: &str, reading: &str) -> Span {
        Span::Pronunciation {
            surface: surface.to_string(),
            reading: reading.to_string(),
        }
    }

    // ---- plain-text renderer --------------------------------------------

    #[test]
    fn plain_text_empty() {
        let u = Utterance { spans: vec![] };
        assert_eq!(render_plain_text(&u), "");
    }

    #[test]
    fn plain_text_reading_replaces_surface() {
        let u = parse_furigana("日本語[にほんご]を").unwrap();
        assert_eq!(render_plain_text(&u), "にほんごを");
    }

    #[test]
    fn plain_text_handles_mid_word_split() {
        let u = parse_furigana("転がり込[こ]んだ").unwrap();
        assert_eq!(render_plain_text(&u), "転がりこんだ");
    }

    #[test]
    fn plain_text_preserves_whitespace_verbatim() {
        let u = parse_furigana("私[わたし] が この 仕事[しごと] を").unwrap();
        assert_eq!(render_plain_text(&u), "わたし が この しごと を");
    }

    // ---- ssml renderer --------------------------------------------------

    const VOICE: &str = "ja-JP-MasaruMultilingualNeural";

    #[test]
    fn ssml_empty_body() {
        let u = Utterance { spans: vec![] };
        let out = render_ssml(&u, VOICE);
        assert_eq!(
            out,
            "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\" \
             xml:lang=\"ja-JP\"><voice name=\"ja-JP-MasaruMultilingualNeural\"></voice></speak>"
        );
    }

    #[test]
    fn ssml_single_pronunciation() {
        let u = Utterance {
            spans: vec![pron("日本語", "にほんご"), text("を")],
        };
        let out = render_ssml(&u, VOICE);
        assert_eq!(
            out,
            "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\" \
             xml:lang=\"ja-JP\"><voice name=\"ja-JP-MasaruMultilingualNeural\">\
             <sub alias=\"にほんご\">日本語</sub>を</voice></speak>"
        );
    }

    #[test]
    fn ssml_xml_escapes_text_span() {
        // `&` `<` `>` all must be escaped in text. `'` and `"` are fine in
        // element content per XML rules but we leave them as-is there.
        let u = Utterance {
            spans: vec![text("A & B < C > D")],
        };
        let out = render_ssml(&u, VOICE);
        assert!(out.contains("A &amp; B &lt; C &gt; D"));
        assert!(!out.contains("A & B"));
    }

    #[test]
    fn ssml_xml_escapes_surface_and_reading_attrs() {
        // Surfaces can never actually contain XML metachars in Japanese
        // input, but the renderer must escape them anyway so we can never
        // emit broken SSML.
        let u = Utterance {
            spans: vec![pron("A&B", "a\"b")],
        };
        let out = render_ssml(&u, VOICE);
        assert!(out.contains("<sub alias=\"a&quot;b\">A&amp;B</sub>"));
    }

    #[test]
    fn ssml_xml_escapes_voice_attribute() {
        let u = Utterance { spans: vec![] };
        let out = render_ssml(&u, "evil\"voice&name");
        assert!(out.contains("name=\"evil&quot;voice&amp;name\""));
    }

    #[test]
    fn ssml_real_sample_exact_output() {
        // Lock the exact expected SSML for the first phase-1 deck sample.
        // This is what Azure Neural TTS will receive.
        let u =
            parse_furigana("日本語[にほんご]を どれくらい 勉強[べんきょう]していますか？").unwrap();
        let out = render_ssml(&u, VOICE);
        let expected = "<speak version=\"1.0\" xmlns=\"http://www.w3.org/2001/10/synthesis\" \
            xml:lang=\"ja-JP\"><voice name=\"ja-JP-MasaruMultilingualNeural\">\
            <sub alias=\"にほんご\">日本語</sub>を どれくらい \
            <sub alias=\"べんきょう\">勉強</sub>していますか？</voice></speak>";
        assert_eq!(out, expected);
    }

    #[test]
    fn ssml_mid_word_split_sample() {
        let u = parse_furigana("転がり込[こ]んだ").unwrap();
        let out = render_ssml(&u, VOICE);
        assert!(out.contains("転がり<sub alias=\"こ\">込</sub>んだ"));
    }

    #[test]
    fn edge_ssml_uses_plain_readings_inside_prosody() {
        let u = parse_furigana("日本語[にほんご]を").unwrap();
        let out = render_edge_ssml(&u, "ja-JP-NanamiNeural", Some(1.25));
        assert!(out.contains("name=\"ja-JP-NanamiNeural\""));
        assert!(out.contains("rate=\"+25%\""));
        assert!(out.contains(">にほんごを</prosody>"));
        assert!(!out.contains("<sub"));
    }

    #[test]
    fn edge_ssml_escapes_text_and_attributes() {
        let u = Utterance {
            spans: vec![text("A & B < C > D")],
        };
        let out = render_edge_ssml(&u, "evil\"voice&name", None);
        assert!(out.contains("name=\"evil&quot;voice&amp;name\""));
        assert!(out.contains("rate=\"default\""));
        assert!(out.contains("A &amp; B &lt; C &gt; D"));
    }

    #[test]
    fn edge_rate_mapping_is_deterministic() {
        let cases = [
            (None, "default"),
            (Some(1.0), "default"),
            (Some(0.75), "-25%"),
            (Some(1.25), "+25%"),
            (Some(1.333), "+33%"),
            (Some(3.0), "+100%"),
            (Some(-1.0), "-100%"),
        ];
        for (input, expected) in cases {
            assert_eq!(edge_rate(input), expected);
        }
    }
}
