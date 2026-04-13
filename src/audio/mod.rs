//! Cross-platform audio playback by shelling out to system binaries.
//!
//! We avoid pulling in a native Rust audio stack (`rodio`, `cpal`, etc.)
//! because Anki-style media can be mp3/ogg/wav/m4a/aac, the deps drag in
//! ALSA/CoreAudio + C codec libraries, and the TUI only needs "play this
//! file, maybe stop it." A dedicated playback thread owns the
//! `std::process::Child` handle and reaps it through `try_wait` so the
//! ratatui render loop never blocks.

pub mod player;

pub use player::{PlayerBinary, PlayerCommand, PlayerHandle, detect_player_binary, spawn_player};
