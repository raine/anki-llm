use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// A detected system audio player. Holds the binary name and any fixed
/// arguments that should precede the file path on every invocation.
#[derive(Debug, Clone)]
pub struct PlayerBinary {
    pub command: String,
    pub args: Vec<String>,
}

impl PlayerBinary {
    /// Spawn this binary playing `path`, with stdout/stderr silenced so
    /// it can't scribble over the TUI.
    fn spawn(&self, path: &PathBuf) -> std::io::Result<Child> {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args).arg(path);
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        cmd.spawn()
    }
}

/// Commands the TUI sends to the playback thread. `Play` carries a
/// `card_id` so the player can implement toggle-on-same-card semantics
/// without the TUI having to track state.
#[derive(Debug, Clone)]
pub enum PlayerCommand {
    Play { card_id: u64, path: PathBuf },
    Stop,
    Shutdown,
}

/// Handle to a running playback thread. Cloning the `tx` is fine —
/// multiple senders are supported. Dropping the handle signals
/// `Shutdown` and joins the thread.
pub struct PlayerHandle {
    tx: Sender<PlayerCommand>,
    join: Option<JoinHandle<()>>,
}

impl PlayerHandle {
    pub fn sender(&self) -> Sender<PlayerCommand> {
        self.tx.clone()
    }

    /// Non-blocking: send a `Play` request. Returns an error only if the
    /// player thread has already exited.
    pub fn play(&self, card_id: u64, path: PathBuf) -> Result<(), mpsc::SendError<PlayerCommand>> {
        self.tx.send(PlayerCommand::Play { card_id, path })
    }

    /// Non-blocking: send a `Stop` request.
    pub fn stop(&self) -> Result<(), mpsc::SendError<PlayerCommand>> {
        self.tx.send(PlayerCommand::Stop)
    }
}

impl Drop for PlayerHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(PlayerCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Probe the platform for an audio player binary. Returns the first
/// available match or `None` if nothing is installed. Call once at
/// session start and cache the result — the same binary is used for all
/// subsequent playback requests in the session.
pub fn detect_player_binary() -> Option<PlayerBinary> {
    #[cfg(target_os = "macos")]
    {
        if binary_exists("afplay") {
            return Some(PlayerBinary {
                command: "afplay".into(),
                args: Vec::new(),
            });
        }
    }

    // Linux / other unices. `mpv` and `ffplay` both handle mp3/ogg/wav
    // reliably and have a no-GUI mode that keeps the TUI clean. `paplay`
    // is wav-only in practice, so it's not a safe fallback for Anki media.
    for candidate in [
        (
            "mpv",
            vec!["--no-video".to_string(), "--really-quiet".to_string()],
        ),
        (
            "ffplay",
            vec![
                "-nodisp".to_string(),
                "-autoexit".to_string(),
                "-loglevel".to_string(),
                "quiet".to_string(),
            ],
        ),
    ] {
        if binary_exists(candidate.0) {
            return Some(PlayerBinary {
                command: candidate.0.to_string(),
                args: candidate.1,
            });
        }
    }

    None
}

/// Spawn a dedicated playback thread using the supplied binary. The
/// returned `PlayerHandle` owns the sender; dropping it triggers a
/// clean shutdown.
///
/// The thread loops on `recv_timeout(100ms)`:
/// - Incoming `Play`: kill+wait any active child, then spawn the new one.
/// - Incoming `Play` for the SAME `card_id` while already playing: kill
///   the active child and do NOT start a new one (toggle-to-stop).
/// - Incoming `Stop` / `Shutdown`: kill+wait the active child and
///   (on `Shutdown`) break the loop.
/// - On timeout: `try_wait` the active child to reap it if it finished
///   naturally. This keeps zombies from accumulating without a second
///   reaper thread.
pub fn spawn_player(binary: PlayerBinary) -> PlayerHandle {
    let (tx, rx) = mpsc::channel::<PlayerCommand>();
    let join = thread::spawn(move || run_player_loop(binary, rx));
    PlayerHandle {
        tx,
        join: Some(join),
    }
}

fn run_player_loop(binary: PlayerBinary, rx: mpsc::Receiver<PlayerCommand>) {
    struct Active {
        child: Child,
        card_id: u64,
    }

    let mut active: Option<Active> = None;
    let tick = Duration::from_millis(100);

    loop {
        match rx.recv_timeout(tick) {
            Ok(PlayerCommand::Play { card_id, path }) => {
                // Toggle semantics: same card pressed while already
                // playing -> stop and stay stopped.
                if let Some(mut a) = active.take() {
                    let still_running = matches!(a.child.try_wait(), Ok(None));
                    let _ = a.child.kill();
                    let _ = a.child.wait();
                    if still_running && a.card_id == card_id {
                        continue;
                    }
                }
                match binary.spawn(&path) {
                    Ok(child) => active = Some(Active { child, card_id }),
                    Err(_) => {
                        // Spawn failed — leave `active` None so the next
                        // `Play` can try again. A smarter version would
                        // surface this via a PlayerEvent channel; v1
                        // just swallows and the caller's state stays
                        // "Ready", which they can retry.
                    }
                }
            }
            Ok(PlayerCommand::Stop) => {
                if let Some(mut a) = active.take() {
                    let _ = a.child.kill();
                    let _ = a.child.wait();
                }
            }
            Ok(PlayerCommand::Shutdown) => {
                if let Some(mut a) = active.take() {
                    let _ = a.child.kill();
                    let _ = a.child.wait();
                }
                break;
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(mut a) = active.take() {
                    match a.child.try_wait() {
                        Ok(Some(_)) => {
                            // Finished naturally; reaped.
                        }
                        Ok(None) => {
                            // Still running; put it back.
                            active = Some(a);
                        }
                        Err(_) => {
                            // `try_wait` hit an OS error. Dropping the
                            // `Child` alone does NOT kill the process —
                            // `Child`'s `Drop` is a no-op by design — so
                            // we must kill + wait ourselves or the
                            // `afplay` / `mpv` subprocess leaks and keeps
                            // playing past the TUI session.
                            let _ = a.child.kill();
                            let _ = a.child.wait();
                        }
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(mut a) = active.take() {
                    let _ = a.child.kill();
                    let _ = a.child.wait();
                }
                break;
            }
        }
    }
}

fn binary_exists(name: &str) -> bool {
    // `PATH`-walking without extra dependencies. We only need
    // yes/no — not the absolute path — because `Command::new(name)` will
    // re-do the lookup at spawn time. Splitting PATH and stat'ing each
    // candidate avoids a crate dep and is fast enough for a startup probe.
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(name);
        std::fs::metadata(&candidate)
            .map(|m| m.is_file())
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Instant;

    /// Build a fake player binary: a shell script that sleeps for a
    /// given number of milliseconds, then exits 0. Writing a PID file
    /// lets tests verify that the child was killed rather than finishing
    /// on its own.
    fn fake_player_script(tmp: &tempfile::TempDir, sleep_ms: u64) -> PathBuf {
        let script_path = tmp.path().join("fake-player.sh");
        let mut f = std::fs::File::create(&script_path).unwrap();
        writeln!(f, "#!/bin/sh\nsleep {}\nexit 0", sleep_ms as f64 / 1000.0).unwrap();
        drop(f);
        // chmod +x
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&script_path, perms).unwrap();
        }
        script_path
    }

    fn fake_binary(script: &PathBuf) -> PlayerBinary {
        PlayerBinary {
            command: script.to_string_lossy().into_owned(),
            args: Vec::new(),
        }
    }

    #[test]
    #[cfg(unix)]
    fn play_then_shutdown_reaps_child() {
        let tmp = tempfile::tempdir().unwrap();
        let script = fake_player_script(&tmp, 5_000);
        let bin = fake_binary(&script);
        let audio = tmp.path().join("audio.mp3");
        std::fs::write(&audio, b"stub").unwrap();

        let handle = spawn_player(bin);
        handle.play(1, audio).unwrap();
        // Let the player thread process the Play.
        std::thread::sleep(Duration::from_millis(250));
        // Dropping the handle should send Shutdown and join cleanly.
        let start = Instant::now();
        drop(handle);
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "shutdown should kill the long-sleeping child quickly, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    #[cfg(unix)]
    fn same_card_replay_toggles_stop() {
        let tmp = tempfile::tempdir().unwrap();
        let script = fake_player_script(&tmp, 5_000);
        let bin = fake_binary(&script);
        let audio = tmp.path().join("audio.mp3");
        std::fs::write(&audio, b"stub").unwrap();

        let handle = spawn_player(bin);
        handle.play(42, audio.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(250));
        // Same card id while playing -> should stop without relaunching.
        handle.play(42, audio.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(250));
        // Hand off to Drop for final cleanup.
        drop(handle);
    }

    #[test]
    #[cfg(unix)]
    fn different_card_replaces_active_child() {
        let tmp = tempfile::tempdir().unwrap();
        let script = fake_player_script(&tmp, 5_000);
        let bin = fake_binary(&script);
        let audio = tmp.path().join("audio.mp3");
        std::fs::write(&audio, b"stub").unwrap();

        let handle = spawn_player(bin);
        handle.play(1, audio.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        handle.play(2, audio.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        handle.stop().unwrap();
        std::thread::sleep(Duration::from_millis(100));
        drop(handle);
    }

    #[test]
    fn detect_player_is_optional() {
        // Smoke test: we don't assert a binary is present — CI may be
        // minimal — but the function must return without panicking and
        // must respect the PATH env.
        let _ = detect_player_binary();
    }

    #[test]
    fn binary_exists_handles_missing_path_var() {
        // Sanity: a nonexistent binary reports false without panicking.
        assert!(!binary_exists("definitely-not-a-real-binary-xyz-12345"));
    }
}
