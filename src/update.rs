use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

const REPO: &str = "raine/anki-llm";
const BIN_NAME: &str = "anki-llm";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn platform_suffix() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("macos", "x86_64") => Ok("darwin-amd64"),
        ("linux", "x86_64") => Ok("linux-amd64"),
        (os, arch) => bail!("Unsupported platform: {os}/{arch}"),
    }
}

fn is_homebrew_install(exe_path: &Path) -> bool {
    let path_str = exe_path.to_string_lossy();
    path_str.contains("/Cellar/")
}

fn fetch_latest_version() -> Result<String> {
    let output = Command::new("curl")
        .args([
            "-sSf",
            "--connect-timeout",
            "10",
            "--max-time",
            "30",
            &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        ])
        .output()
        .context("Failed to run curl")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("Failed to fetch latest release: {}", stderr.trim());
    }

    let body: serde_json::Value =
        serde_json::from_slice(&output.stdout).context("Failed to parse GitHub API response")?;

    let tag = body["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("No tag_name in GitHub API response"))?;

    Ok(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

fn download(url: &str, dest: &Path) -> Result<()> {
    let status = Command::new("curl")
        .args([
            "-sSLf",
            "--connect-timeout",
            "10",
            "--max-time",
            "120",
            "-o",
        ])
        .arg(dest)
        .arg(url)
        .status()
        .context("Failed to run curl")?;

    if !status.success() {
        bail!("Download failed: {url}");
    }
    Ok(())
}

fn extract_tar(archive: &Path, dest: &Path) -> Result<()> {
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(dest)
        .status()
        .context("Failed to run tar")?;

    if !status.success() {
        bail!("Failed to extract archive");
    }
    Ok(())
}

fn sha256_of(path: &Path) -> Result<String> {
    if let Ok(output) = Command::new("sha256sum").arg(path).output()
        && output.status.success()
    {
        let out = String::from_utf8_lossy(&output.stdout);
        if let Some(hash) = out.split_whitespace().next() {
            return Ok(hash.to_string());
        }
    }

    let output = Command::new("shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .context("Neither sha256sum nor shasum found. Cannot verify checksum")?;

    if !output.status.success() {
        bail!("Checksum command failed");
    }

    let out = String::from_utf8_lossy(&output.stdout);
    out.split_whitespace()
        .next()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("Could not parse checksum output"))
}

fn verify_checksum(file: &Path, expected_line: &str) -> Result<()> {
    let expected_hash = expected_line
        .split_whitespace()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Invalid checksum file format"))?;

    let actual_hash = sha256_of(file)?;
    if actual_hash != expected_hash {
        bail!("Checksum mismatch!\n  Expected: {expected_hash}\n  Got:      {actual_hash}");
    }
    Ok(())
}

fn replace_binary(new_binary: &Path, current_exe: &Path) -> Result<()> {
    let exe_dir = current_exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("Could not determine binary directory"))?;

    let staged = exe_dir.join(format!(".{BIN_NAME}.new"));
    std::fs::copy(new_binary, &staged).context("Failed to copy new binary")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .context("Failed to set permissions")?;
    }

    std::fs::rename(&staged, current_exe).context("Failed to install new binary")?;
    Ok(())
}

fn do_update(
    pb: &indicatif::ProgressBar,
    artifact_name: &str,
    current_exe: &Path,
) -> Result<String> {
    let latest_version = fetch_latest_version()?;

    if latest_version == CURRENT_VERSION {
        return Ok(format!("Already up to date (v{CURRENT_VERSION})"));
    }

    pb.set_message(format!("Downloading v{latest_version}..."));

    let tmp = tempfile::tempdir().context("Failed to create temp directory")?;
    let tar_path = tmp.path().join(format!("{artifact_name}.tar.gz"));
    let sha_path = tmp.path().join(format!("{artifact_name}.sha256"));

    let base_url = format!("https://github.com/{REPO}/releases/download/v{latest_version}");

    download(&format!("{base_url}/{artifact_name}.tar.gz"), &tar_path)?;
    download(&format!("{base_url}/{artifact_name}.sha256"), &sha_path)?;

    pb.set_message("Verifying checksum...");
    let sha_content = std::fs::read_to_string(&sha_path).context("Failed to read checksum file")?;
    verify_checksum(&tar_path, &sha_content)?;

    pb.set_message("Installing...");
    let extract_dir = tmp.path().join("extract");
    std::fs::create_dir(&extract_dir).context("Failed to create extract dir")?;
    extract_tar(&tar_path, &extract_dir)?;

    let new_binary = extract_dir.join(BIN_NAME);
    if !new_binary.exists() {
        bail!("Extracted archive does not contain '{BIN_NAME}' binary");
    }

    replace_binary(&new_binary, current_exe)?;

    Ok(format!(
        "Updated {BIN_NAME} v{CURRENT_VERSION} -> v{latest_version}"
    ))
}

pub fn run() -> Result<()> {
    let current_exe = std::env::current_exe().context("Could not determine executable path")?;

    let canonical_exe = std::fs::canonicalize(&current_exe).unwrap_or(current_exe.clone());
    if is_homebrew_install(&canonical_exe) {
        bail!("anki-llm is managed by Homebrew. Run `brew upgrade anki-llm` instead.");
    }

    let platform = platform_suffix()?;
    let artifact_name = format!("{BIN_NAME}-{platform}");

    let pb = indicatif::ProgressBar::new_spinner();
    pb.set_style(
        indicatif::ProgressStyle::default_spinner()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
            .template("{spinner:.blue} {msg}")
            .unwrap(),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(120));
    pb.set_message("Checking for updates...");

    match do_update(&pb, &artifact_name, &canonical_exe) {
        Ok(msg) => {
            pb.finish_with_message(format!("✔ {msg}"));
            Ok(())
        }
        Err(e) => {
            pb.finish_with_message("✘ Update failed".to_string());
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_platform_suffix_current() {
        let suffix = platform_suffix().unwrap();
        assert!(
            ["darwin-arm64", "darwin-amd64", "linux-amd64"].contains(&suffix),
            "unexpected platform suffix: {suffix}"
        );
    }

    #[test]
    fn test_is_homebrew_cellar() {
        assert!(is_homebrew_install(Path::new(
            "/opt/homebrew/Cellar/anki-llm/0.1.42/bin/anki-llm"
        )));
    }

    #[test]
    fn test_is_homebrew_prefix() {
        assert!(is_homebrew_install(Path::new(
            "/usr/local/Cellar/anki-llm/0.1.42/bin/anki-llm"
        )));
    }

    #[test]
    fn test_is_not_homebrew_local_bin() {
        assert!(!is_homebrew_install(Path::new("/usr/local/bin/anki-llm")));
    }

    #[test]
    fn test_is_not_homebrew_home() {
        assert!(!is_homebrew_install(Path::new(
            "/home/user/.local/bin/anki-llm"
        )));
    }
}
