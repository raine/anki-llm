//! Voice browser TUI: `anki-llm tts voices` / `anki-llm tts-voices`.
//!
//! Lets users fuzzy-search the full catalog of OpenAI / Azure / Google /
//! Polly voices, audition the highlighted voice with a short pangram
//! sample, and copy a pasteable `tts:` YAML block to the clipboard on Enter.
//!
//! Design notes live in `history/2026-04-13-plan-tts-voices-tui.md`.

pub mod app;
pub mod catalog;
pub mod command;
pub mod credentials;
pub mod player;
pub mod preview;
pub mod sample;
pub mod yaml;
