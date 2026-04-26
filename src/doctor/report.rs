#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    Warn,
    Fail,
    Skip,
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub label: String,
    pub status: Status,
    pub detail: Option<String>,
}

impl CheckResult {
    pub fn ok(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Ok,
            detail: Some(detail.into()),
        }
    }

    pub fn warn(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Warn,
            detail: Some(detail.into()),
        }
    }

    pub fn fail(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Fail,
            detail: Some(detail.into()),
        }
    }

    pub fn skip(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: Status::Skip,
            detail: Some(detail.into()),
        }
    }
}

/// Mask a secret for display: show first 2 and last 4 characters.
pub fn mask(value: &str) -> String {
    let v = value.trim();
    let chars: Vec<char> = v.chars().collect();
    match chars.len() {
        0 => String::new(),
        1..=6 => "***".to_string(),
        n => {
            let head: String = chars[..2].iter().collect();
            let tail: String = chars[n - 4..].iter().collect();
            format!("{head}…{tail}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_short() {
        assert_eq!(mask(""), "");
        assert_eq!(mask("abc"), "***");
        assert_eq!(mask("abcdef"), "***");
    }

    #[test]
    fn mask_long() {
        assert_eq!(mask("sk-abcdef1234"), "sk…1234");
    }
}
