use std::io::IsTerminal;
use std::sync::OnceLock;

static STYLE: OnceLock<Style> = OnceLock::new();

/// Returns the global `Style` instance, initialized once based on TTY detection.
pub fn style() -> &'static Style {
    STYLE.get_or_init(Style::detect)
}

/// ANSI styling helper. Disabled when stderr is not a TTY or `NO_COLOR` is set.
#[derive(Clone, Copy)]
pub struct Style {
    pub enabled: bool,
}

impl Style {
    fn detect() -> Self {
        let enabled = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
        Self { enabled }
    }

    fn paint(&self, code: &str, text: impl std::fmt::Display) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    pub fn bold(&self, text: impl std::fmt::Display) -> String {
        self.paint("1", text)
    }
    pub fn dim(&self, text: impl std::fmt::Display) -> String {
        self.paint("2", text)
    }
    pub fn green(&self, text: impl std::fmt::Display) -> String {
        self.paint("32", text)
    }
    pub fn cyan(&self, text: impl std::fmt::Display) -> String {
        self.paint("36", text)
    }
    pub fn yellow(&self, text: impl std::fmt::Display) -> String {
        self.paint("33", text)
    }
    pub fn red(&self, text: impl std::fmt::Display) -> String {
        self.paint("31", text)
    }

    // Semantic helpers
    pub fn success(&self, text: impl std::fmt::Display) -> String {
        self.paint("1;32", text)
    }
    pub fn warning(&self, text: impl std::fmt::Display) -> String {
        self.paint("33", text)
    }
    pub fn error_text(&self, text: impl std::fmt::Display) -> String {
        self.paint("1;31", text)
    }
    pub fn accent(&self, text: impl std::fmt::Display) -> String {
        self.paint("1;36", text)
    }
    pub fn muted(&self, text: impl std::fmt::Display) -> String {
        self.paint("2", text)
    }
}
