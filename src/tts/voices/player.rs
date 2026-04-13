//! Spawn a system audio player as a detached child. The TUI owns the
//! `Child` so it can kill playback on the next Space / Enter / Esc.
//!
//! Every candidate binary is launched with null stdin/stdout/stderr
//! so it can't corrupt the ratatui alt-screen.

use std::path::Path;
use std::process::{Child, Command, Stdio};

#[cfg(target_os = "macos")]
pub fn spawn(path: &Path) -> Result<Child, String> {
    spawn_silent("afplay", &[path.to_string_lossy().as_ref()])
}

#[cfg(target_os = "linux")]
pub fn spawn(path: &Path) -> Result<Child, String> {
    let p = path.to_string_lossy();
    let candidates: &[(&str, &[&str])] = &[
        ("mpv", &["--really-quiet", "--no-video", &p]),
        (
            "ffplay",
            &["-nodisp", "-autoexit", "-loglevel", "quiet", &p],
        ),
        ("paplay", &[&p]),
    ];
    for (bin, args) in candidates {
        match spawn_silent(bin, args) {
            Ok(child) => return Ok(child),
            Err(_) => continue,
        }
    }
    Err("no audio player found (install mpv, ffplay, or paplay)".into())
}

#[cfg(target_os = "windows")]
pub fn spawn(path: &Path) -> Result<Child, String> {
    let script = format!(
        "(New-Object Media.SoundPlayer \"{}\").PlaySync();",
        path.display()
    );
    spawn_silent("powershell", &["-NoProfile", "-Command", &script])
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn spawn(_path: &Path) -> Result<Child, String> {
    Err("audio playback not supported on this OS".into())
}

fn spawn_silent(bin: &str, args: &[&str]) -> Result<Child, String> {
    Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{bin}: {e}"))
}

/// Best-effort kill: send SIGKILL (or equivalent) and reap the child.
/// Swallows errors — the caller just wants the audio to stop.
pub fn stop(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}
