//! Semantic intermediate representation for TTS input.
//!
//! Anki fields for Japanese decks commonly store inline furigana using the
//! convention `Kanji[reading]`, e.g. `日本語[にほんご]を 勉強[べんきょう]`.
//! Different TTS providers need different renderings of that information:
//!
//! - OpenAI TTS gets plain kana (the kanji surface dropped in favor of the
//!   reading), because it does not understand Anki ruby markup.
//! - Azure Neural TTS gets SSML `<sub alias="READING">SURFACE</sub>`
//!   substitutions, which give it an explicit pronunciation hint while
//!   preserving the kanji surface for voice models that can use it.
//!
//! Both paths start from the same `Utterance` built by `parse_furigana`. The
//! parser walks the input and binds `[reading]` annotations to the
//! immediately-preceding run of Han (CJK) characters. This differs from the
//! older HyperTTS whitespace-delimited regex, which over-matched on
//! mid-word bracket splits like `転がり込[こ]んだ`.

use thiserror::Error;

/// A single TTS utterance, composed of a sequence of spans. Each span is
/// either plain text or a pronunciation annotation binding a surface form
/// (kanji cluster) to its reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utterance {
    pub spans: Vec<Span>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Span {
    /// Plain text. Includes non-annotated kanji, kana, punctuation, Latin
    /// letters, whitespace, etc.
    Text(String),
    /// A kanji cluster explicitly annotated with its reading.
    Pronunciation { surface: String, reading: String },
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    #[error("unmatched '[' at byte offset {0}")]
    UnclosedAnnotation(usize),
    #[error("empty reading at byte offset {0}")]
    EmptyReading(usize),
    #[error("annotation at byte offset {0} is not preceded by a kanji cluster")]
    OrphanAnnotation(usize),
    #[error("reading '{reading}' contains non-kana character '{ch}'")]
    NonKanaReading { reading: String, ch: char },
}

/// True for characters that can appear as the surface form before a
/// `[reading]` annotation: the main CJK Unified Ideographs block,
/// Extension A, Compatibility Ideographs, iteration/abbreviation marks
/// (`々 〆 〇 ヶ ヵ`), and the full Katakana block.
///
/// Katakana is included because loanwords written in katakana (e.g.
/// `スパイク`) sometimes carry reading annotations in learner decks, and
/// rejecting them breaks TTS preparation for otherwise valid cards.
///
/// Note: Katakana `ケ` and `カ` (`ヶ`'s ancestors) are NOT treated as cluster
/// chars — only the small-form `ヶ` and `ヵ`, which function as counters
/// bound to numerals.
fn is_surface_char(c: char) -> bool {
    matches!(c,
        '\u{3400}'..='\u{4DBF}'   // CJK Unified Ideographs Extension A
        | '\u{4E00}'..='\u{9FFF}' // CJK Unified Ideographs
        | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
        | '々' | '〆' | '〇' | 'ヶ' | 'ヵ'
        | '\u{30A0}'..='\u{30FF}' // Katakana
    )
}

/// True for characters that may legitimately appear inside a reading. We
/// keep this conservative: the full Hiragana and Katakana blocks (which
/// already include the prolonged-sound mark `ー`, middle dot `・`, and
/// kana iteration marks `ゝゞヽヾ`). Real decks occasionally use the middle
/// dot to separate foreign-word syllables inside a reading.
fn is_reading_char(c: char) -> bool {
    matches!(c,
        '\u{3040}'..='\u{309F}'   // Hiragana block
        | '\u{30A0}'..='\u{30FF}' // Katakana block
    )
}

/// Parse an inline-furigana Anki field value into an `Utterance`.
///
/// The parser is a single pass over the chars:
///
/// 1. Accumulate plain text.
/// 2. When a kanji-cluster character appears, peek ahead through the rest
///    of the cluster and check whether the first non-cluster char is `[`.
///    If yes, the cluster becomes a `Pronunciation`'s surface; if no, it
///    stays plain text.
/// 3. Inside a `[...]` annotation, scan until the matching `]`. Validate
///    the reading, emit the span, resume text accumulation.
///
/// Malformed input produces a `ParseError`. Orphan annotations (no
/// preceding cluster), empty readings, non-kana readings, and unclosed
/// brackets are all rejected.
pub fn parse_furigana(input: &str) -> Result<Utterance, ParseError> {
    let mut spans: Vec<Span> = Vec::new();
    let mut text_buf = String::new();
    let mut chars = input.char_indices().peekable();

    while let Some((idx, c)) = chars.next() {
        if c == '[' {
            // Take the trailing kanji cluster out of `text_buf`. Everything
            // to the left stays as a Text span.
            let cluster = take_trailing_cluster(&mut text_buf);
            if cluster.is_empty() {
                return Err(ParseError::OrphanAnnotation(idx));
            }
            if !text_buf.is_empty() {
                spans.push(Span::Text(std::mem::take(&mut text_buf)));
            }

            // Scan the reading up to ']'.
            let mut reading = String::new();
            let mut closed = false;
            for (_, rc) in chars.by_ref() {
                if rc == ']' {
                    closed = true;
                    break;
                }
                reading.push(rc);
            }
            if !closed {
                return Err(ParseError::UnclosedAnnotation(idx));
            }
            if reading.is_empty() {
                return Err(ParseError::EmptyReading(idx));
            }
            if let Some(bad) = reading.chars().find(|c| !is_reading_char(*c)) {
                return Err(ParseError::NonKanaReading {
                    reading: reading.clone(),
                    ch: bad,
                });
            }

            spans.push(Span::Pronunciation {
                surface: cluster,
                reading,
            });
        } else {
            text_buf.push(c);
        }
    }

    if !text_buf.is_empty() {
        spans.push(Span::Text(text_buf));
    }
    Ok(Utterance { spans })
}

/// Pop the trailing run of Han-cluster characters off the end of `buf` and
/// return it. If the last char is not a cluster char, returns an empty
/// `String` and leaves `buf` untouched.
fn take_trailing_cluster(buf: &mut String) -> String {
    // Walk backward char-by-char to find the start of the trailing cluster.
    let split_at = {
        let mut split = buf.len();
        for (idx, ch) in buf.char_indices().rev() {
            if is_surface_char(ch) {
                split = idx;
            } else {
                break;
            }
        }
        split
    };
    let cluster: String = buf[split_at..].to_string();
    buf.truncate(split_at);
    cluster
}

impl Utterance {
    /// Stable, rebuild-safe serialization used as cache-key input. Two
    /// utterances with identical span sequences always produce the same
    /// string, regardless of how the original input was formatted.
    pub fn canonical(&self) -> String {
        let mut out = String::new();
        for span in &self.spans {
            match span {
                Span::Text(t) => {
                    out.push_str("T:");
                    out.push_str(t);
                    out.push('\n');
                }
                Span::Pronunciation { surface, reading } => {
                    out.push_str("P:");
                    out.push_str(surface);
                    out.push('|');
                    out.push_str(reading);
                    out.push('\n');
                }
            }
        }
        out
    }

    /// True if the utterance has no spans, or only empty text spans.
    pub fn is_empty(&self) -> bool {
        self.spans.iter().all(|s| match s {
            Span::Text(t) => t.is_empty(),
            Span::Pronunciation { .. } => false,
        })
    }
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn empty_input() {
        let u = parse_furigana("").unwrap();
        assert!(u.spans.is_empty());
        assert!(u.is_empty());
    }

    #[test]
    fn plain_text_no_annotations() {
        let u = parse_furigana("hello world").unwrap();
        assert_eq!(u.spans, vec![text("hello world")]);
    }

    #[test]
    fn simple_single_annotation() {
        let u = parse_furigana("日本語[にほんご]を").unwrap();
        assert_eq!(u.spans, vec![pron("日本語", "にほんご"), text("を")]);
    }

    #[test]
    fn mid_word_annotation_is_han_cluster_only() {
        // The whole point of the Han-cluster parser: annotation binds to
        // the trailing kanji run (`込`), not to the space-delimited token
        // `転がり込`. The hiragana `転がり` stays plain text.
        let u = parse_furigana("転がり込[こ]んだ").unwrap();
        assert_eq!(
            u.spans,
            vec![text("転がり"), pron("込", "こ"), text("んだ")]
        );
    }

    #[test]
    fn honorific_prefix_is_text() {
        let u = parse_furigana("お父[とう]さん").unwrap();
        assert_eq!(u.spans, vec![text("お"), pron("父", "とう"), text("さん")]);
    }

    #[test]
    fn iteration_mark_inside_cluster() {
        let u = parse_furigana("人々[ひとびと]").unwrap();
        assert_eq!(u.spans, vec![pron("人々", "ひとびと")]);
    }

    #[test]
    fn small_ke_inside_cluster() {
        let u = parse_furigana("三ヶ月[さんかげつ]").unwrap();
        assert_eq!(u.spans, vec![pron("三ヶ月", "さんかげつ")]);
    }

    #[test]
    fn small_ka_inside_cluster() {
        let u = parse_furigana("三ヵ月[さんかげつ]").unwrap();
        assert_eq!(u.spans, vec![pron("三ヵ月", "さんかげつ")]);
    }

    #[test]
    fn compat_ideograph_in_cluster() {
        // `﨑` is CJK compat ideograph U+FA11, common in Japanese names.
        let u = parse_furigana("山﨑[やまさき]さん").unwrap();
        assert_eq!(u.spans, vec![pron("山﨑", "やまさき"), text("さん")]);
    }

    #[test]
    fn legacy_hypertts_spaced_input() {
        let u = parse_furigana("私[わたし] が この 仕事[しごと] を").unwrap();
        assert_eq!(
            u.spans,
            vec![
                pron("私", "わたし"),
                text(" が この "),
                pron("仕事", "しごと"),
                text(" を"),
            ]
        );
    }

    #[test]
    fn deck_prompt_correct_example() {
        // Verbatim from the user's deck prompt. The Han-cluster parser
        // must handle this exactly as-is.
        let u = parse_furigana("彼[かれ]は うちに 転[ころ] がり 込[こ]んだ。").unwrap();
        assert_eq!(
            u.spans,
            vec![
                pron("彼", "かれ"),
                text("は うちに "),
                pron("転", "ころ"),
                text(" がり "),
                pron("込", "こ"),
                text("んだ。"),
            ]
        );
    }

    #[test]
    fn deck_prompt_incorrect_style_handled_natively() {
        // The "incorrect" style from the user's deck prompt was incorrect
        // only for HyperTTS. Our Han-cluster parser handles it natively.
        let u = parse_furigana("転がり込[こ]んだ").unwrap();
        assert_eq!(
            u.spans,
            vec![text("転がり"), pron("込", "こ"), text("んだ")]
        );
    }

    #[test]
    fn katakana_reading() {
        let u = parse_furigana("明太子[メンタイコ]").unwrap();
        assert_eq!(u.spans, vec![pron("明太子", "メンタイコ")]);
    }

    #[test]
    fn katakana_surface_with_reading() {
        // Loanwords in katakana sometimes carry hiragana readings in
        // learner decks. The parser must not reject them as orphan
        // annotations.
        let u = parse_furigana("スパイク[すぱいく]が").unwrap();
        assert_eq!(u.spans, vec![pron("スパイク", "すぱいく"), text("が")]);
    }

    #[test]
    fn long_vowel_mark_in_reading() {
        let u = parse_furigana("珈琲[コーヒー]").unwrap();
        assert_eq!(u.spans, vec![pron("珈琲", "コーヒー")]);
    }

    #[test]
    fn unclosed_bracket_is_error() {
        assert_eq!(
            parse_furigana("日本[にほん"),
            Err(ParseError::UnclosedAnnotation(6)) // `日本` = 6 bytes
        );
    }

    #[test]
    fn orphan_annotation_rejected() {
        let err = parse_furigana("[foo]").unwrap_err();
        assert!(matches!(err, ParseError::OrphanAnnotation(_)));
    }

    #[test]
    fn orphan_annotation_after_kana_rejected() {
        // Kana only → no trailing cluster before the bracket.
        let err = parse_furigana("ひらがな[reading]").unwrap_err();
        assert!(matches!(err, ParseError::OrphanAnnotation(_)));
    }

    #[test]
    fn empty_reading_rejected() {
        let err = parse_furigana("日本[]").unwrap_err();
        assert!(matches!(err, ParseError::EmptyReading(_)));
    }

    #[test]
    fn non_kana_reading_rejected() {
        let err = parse_furigana("日本[nihon]").unwrap_err();
        match err {
            ParseError::NonKanaReading { ch, .. } => assert_eq!(ch, 'n'),
            _ => panic!("wrong error variant: {err:?}"),
        }
    }

    #[test]
    fn stray_closing_bracket_stays_text() {
        // A ']' outside an annotation context is not a syntax error — the
        // parser only reacts to '['. Downstream rendering will include it
        // verbatim as text.
        let u = parse_furigana("foo]bar").unwrap();
        assert_eq!(u.spans, vec![text("foo]bar")]);
    }

    #[test]
    fn canonical_is_stable_across_equivalent_inputs() {
        // Two different input formats with the same IR produce the same
        // canonical string.
        let a = Utterance {
            spans: vec![pron("日本語", "にほんご"), text("を")],
        };
        let b = parse_furigana("日本語[にほんご]を").unwrap();
        assert_eq!(a.canonical(), b.canonical());
    }

    #[test]
    fn canonical_distinguishes_different_readings() {
        let a = Utterance {
            spans: vec![pron("人", "ひと")],
        };
        let b = Utterance {
            spans: vec![pron("人", "じん")],
        };
        assert_ne!(a.canonical(), b.canonical());
    }

    #[test]
    fn canonical_distinguishes_pron_from_text() {
        let a = Utterance {
            spans: vec![pron("人", "ひと")],
        };
        let b = Utterance {
            spans: vec![text("人ひと")],
        };
        assert_ne!(a.canonical(), b.canonical());
    }

    /// Five real deck samples from `history/scratch/phase1-azure-ssml/test.py`.
    /// These are fetched from a real `Oma dekki` deck and are the exact
    /// strings the phase-1 empirical check confirmed produced good Azure
    /// audio.
    #[test]
    fn real_deck_samples_parse_successfully() {
        let samples = [
            "日本語[にほんご]を どれくらい 勉強[べんきょう]していますか？",
            "どう 言[い]っていいか わからないです。",
            "話[はな]すより 理解[りかい]する 方[ほう]が 得意[とくい]です。",
            "お 手数[てすう]を おかけして すみません。",
            "ひょっとして、 サウナが 幸福[こうふく]を もたらしているという こと？",
        ];
        for s in samples {
            let u = parse_furigana(s).unwrap_or_else(|e| {
                panic!("failed to parse sample {s:?}: {e}");
            });
            assert!(
                u.spans
                    .iter()
                    .any(|sp| matches!(sp, Span::Pronunciation { .. })),
                "sample had no pronunciation spans: {s}"
            );
        }
    }

    #[test]
    fn first_sample_exact_spans() {
        let u =
            parse_furigana("日本語[にほんご]を どれくらい 勉強[べんきょう]していますか？").unwrap();
        assert_eq!(
            u.spans,
            vec![
                pron("日本語", "にほんご"),
                text("を どれくらい "),
                pron("勉強", "べんきょう"),
                text("していますか？"),
            ]
        );
    }
}
