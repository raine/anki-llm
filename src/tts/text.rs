use std::sync::OnceLock;

use regex::Regex;

static HTML_TAG: OnceLock<Regex> = OnceLock::new();
static CLOZE: OnceLock<Regex> = OnceLock::new();
static SOUND_TAG: OnceLock<Regex> = OnceLock::new();
static WHITESPACE: OnceLock<Regex> = OnceLock::new();

fn html_tag() -> &'static Regex {
    HTML_TAG.get_or_init(|| Regex::new(r"<[^>]+>").unwrap())
}
fn cloze() -> &'static Regex {
    // {{c1::answer}} or {{c1::answer::hint}} → "answer"
    CLOZE.get_or_init(|| Regex::new(r"\{\{c\d+::([^:}]*)(?:::[^}]*)?\}\}").unwrap())
}
fn sound_tag() -> &'static Regex {
    SOUND_TAG.get_or_init(|| Regex::new(r"\[sound:[^\]]*\]").unwrap())
}
fn ws() -> &'static Regex {
    WHITESPACE.get_or_init(|| Regex::new(r"\s+").unwrap())
}

/// Normalize a raw Anki field value into text suitable for TTS synthesis.
///
/// Strips HTML tags, cloze markers (`{{c1::x}}` → `x`), existing `[sound:...]`
/// tags, decodes common HTML entities, and collapses whitespace. The output
/// of this function is what the cache keys against and what gets sent to the
/// TTS provider.
pub fn normalize(raw: &str) -> String {
    let t = sound_tag().replace_all(raw, " ");
    let t = cloze().replace_all(&t, "$1");
    let t = html_tag().replace_all(&t, " ");
    let t = decode_entities(&t);
    ws().replace_all(t.trim(), " ").into_owned()
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_unchanged() {
        assert_eq!(normalize("hello world"), "hello world");
    }

    #[test]
    fn empty_input() {
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn strips_html_tags() {
        assert_eq!(normalize("<b>hello</b> <i>world</i>"), "hello world");
    }

    #[test]
    fn strips_cloze() {
        assert_eq!(normalize("{{c1::answer}} more"), "answer more");
        assert_eq!(normalize("{{c1::answer::hint}}"), "answer");
    }

    #[test]
    fn strips_sound_tags() {
        assert_eq!(normalize("[sound:foo.mp3] hello"), "hello");
    }

    #[test]
    fn collapses_whitespace() {
        assert_eq!(normalize("a   b\n\tc"), "a b c");
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(normalize("foo &amp; bar&nbsp;baz"), "foo & bar baz");
    }

    #[test]
    fn combined() {
        let raw = "<div>{{c1::hello}}, <b>world</b></div>[sound:old.mp3]&nbsp;and   more";
        assert_eq!(normalize(raw), "hello, world and more");
    }
}
