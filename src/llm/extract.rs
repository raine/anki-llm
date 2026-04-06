use regex::Regex;
use std::sync::LazyLock;

static RESULT_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<result>(.*?)</result>").unwrap());

/// Extract content from `<result>...</result>` tags in LLM response text.
///
/// If `require` is true and tags are not found, returns an error.
/// If `require` is false and tags are not found, returns the original text.
pub fn extract_result_tag(text: &str, require: bool) -> Result<String, String> {
    if let Some(caps) = RESULT_TAG_RE.captures(text) {
        Ok(caps[1].trim().to_string())
    } else if require {
        Err("response missing required <result></result> tags".to_string())
    } else {
        Ok(text.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_content() {
        let text = "Here is the answer:\n<result>hello world</result>\nDone.";
        assert_eq!(extract_result_tag(text, false).unwrap(), "hello world");
    }

    #[test]
    fn returns_raw_when_not_required() {
        assert_eq!(
            extract_result_tag("no tags here", false).unwrap(),
            "no tags here"
        );
    }

    #[test]
    fn errors_when_required_and_missing() {
        assert!(extract_result_tag("no tags here", true).is_err());
    }

    #[test]
    fn trims_whitespace() {
        let text = "<result>\n  trimmed  \n</result>";
        assert_eq!(extract_result_tag(text, false).unwrap(), "trimmed");
    }
}
