#!/usr/bin/env python3
"""
Release helper for npm package.

Usage:
    # Via npm scripts (recommended):
    pnpm publish:patch -- --otp 123456         # Bump patch version (1.3.0 → 1.3.1)
    pnpm publish:minor -- --otp 123456         # Bump minor version (1.3.0 → 1.4.0)
    pnpm publish:major -- --otp 123456         # Bump major version (1.3.0 → 2.0.0)

    # Direct invocation:
    ./scripts/release.py patch --otp 123456

The script will:
1. Ensure clean working tree
2. Bump version, commit, and tag (via `npm version`)
3. Publish to npm with the provided OTP code
4. Push commits and tags to remote
"""

import argparse
import json
import pathlib
import subprocess
import sys


ROOT = pathlib.Path(__file__).resolve().parent.parent
PACKAGE_JSON = ROOT / "package.json"


def run(cmd: list[str], *, check: bool = True) -> subprocess.CompletedProcess:
    """Execute a command relative to the repo root."""
    return subprocess.run(cmd, cwd=ROOT, check=check)


def run_capture(cmd: list[str]) -> str:
    result = subprocess.run(
        cmd,
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return result.stdout


def ensure_clean_worktree() -> None:
    status = run_capture(["git", "status", "--porcelain", "--ignore-submodules"])
    if status.strip():
        sys.stderr.write("error: working tree must be clean before releasing\n")
        sys.stderr.write(status)
        sys.exit(1)


def read_package_info() -> tuple[str, str]:
    package_data = json.loads(PACKAGE_JSON.read_text())
    name = package_data.get("name")
    version = package_data.get("version")

    if not name or not version:
        sys.stderr.write("error: unable to extract package name/version from package.json\n")
        sys.exit(1)

    return name, version


def version_and_tag(bump: str) -> str:
    """Run npm version which bumps version, commits, and tags."""
    result = run_capture(["npm", "version", bump])
    # npm version returns the new version like "v1.3.1"
    new_version = result.strip().lstrip("v")
    return new_version


def publish_package(otp: str) -> None:
    """Publish to npm with OTP code."""
    run(["npm", "publish", "--otp", otp])


def push_changes() -> None:
    """Push commits and tags to remote."""
    run(["git", "push"])
    run(["git", "push", "--tags"])


def main() -> None:
    parser = argparse.ArgumentParser(description="Release helper for npm package")
    parser.add_argument(
        "bump",
        choices=("patch", "minor", "major"),
        help="Semver component to bump",
    )
    parser.add_argument(
        "--otp",
        required=True,
        help="NPM one-time password for authentication",
    )
    args = parser.parse_args()

    ensure_clean_worktree()

    package_name, current_version = read_package_info()
    print(f"Current version: {current_version}")

    new_version = version_and_tag(args.bump)
    print(f"Bumped to version: {new_version}")

    publish_package(args.otp)

    push_changes()

    print(f"Released {package_name} v{new_version} and pushed to remote")


if __name__ == "__main__":
    main()
