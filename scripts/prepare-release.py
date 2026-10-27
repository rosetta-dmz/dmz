#!/usr/bin/env python3
"""
DMZ Automated Release Preparation Utility

Synchronizes version numbers across:
- Cargo.toml ([workspace.package].version)
- Cargo.lock (via cargo check --workspace)
- dmz-core/workspace/airgap_manifest.json (dmz_version)
- CHANGELOG.md (Conventional Commits changelog generator)
"""

import argparse
import datetime
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT_DIR = Path(__file__).resolve().parent.parent
CARGO_TOML_PATH = ROOT_DIR / "Cargo.toml"
MANIFEST_PATH = ROOT_DIR / "dmz-core" / "workspace" / "airgap_manifest.json"
CHANGELOG_PATH = ROOT_DIR / "CHANGELOG.md"


def get_current_workspace_version() -> str:
    content = CARGO_TOML_PATH.read_text(encoding="utf-8")
    # Match version inside [workspace.package]
    in_workspace_package = False
    for line in content.splitlines():
        trimmed = line.strip()
        if trimmed == "[workspace.package]":
            in_workspace_package = True
            continue
        elif trimmed.startswith("[") and in_workspace_package:
            break
        if in_workspace_package:
            m = re.match(r'^version\s*=\s*"([^"]+)"', trimmed)
            if m:
                return m.group(1)
    raise ValueError(f"Could not find [workspace.package].version in {CARGO_TOML_PATH}")


def parse_semver(version: str) -> tuple[int, int, int]:
    m = re.match(r"^(\d+)\.(\d+)\.(\d+)", version)
    if not m:
        raise ValueError(f"Invalid semver version: {version}")
    return int(m.group(1)), int(m.group(2)), int(m.group(3))


def bump_version(current: str, bump_type: str) -> str:
    major, minor, patch = parse_semver(current)
    if bump_type == "major":
        return f"{major + 1}.0.0"
    elif bump_type == "minor":
        return f"{major}.{minor + 1}.0"
    elif bump_type == "patch":
        return f"{major}.{minor}.{patch + 1}"
    else:
        raise ValueError(f"Unknown bump type: {bump_type}")


def get_latest_git_tag() -> str | None:
    try:
        tag = subprocess.check_output(
            ["git", "describe", "--tags", "--abbrev=0", "--match", "v*"],
            cwd=ROOT_DIR,
            stderr=subprocess.DEVNULL,
            text=True,
        ).strip()
        return tag
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None


def get_commits_since_tag(tag: str | None) -> list[str]:
    rev_range = f"{tag}..HEAD" if tag else "HEAD"
    try:
        output = subprocess.check_output(
            ["git", "log", rev_range, "--no-merges", "--pretty=format:%s"],
            cwd=ROOT_DIR,
            text=True,
        ).strip()
        if not output:
            return []
        return [line.strip() for line in output.splitlines() if line.strip()]
    except subprocess.CalledProcessError:
        return []


def detect_bump_type(commits: list[str]) -> str:
    has_breaking = False
    has_feat = False

    NON_FEATURE_SCOPES = {"ci", "build", "chore", "docs", "test", "infra"}

    for commit in commits:
        if "BREAKING CHANGE:" in commit or re.match(r"^[a-zA-Z]+(\([^\)]+\))?!:", commit):
            has_breaking = True
            break
        m = re.match(r"^feat(?:\(([^)]+)\))?:", commit)
        if m:
            scope = m.group(1).lower() if m.group(1) else ""
            if scope not in NON_FEATURE_SCOPES:
                has_feat = True

    if has_breaking:
        return "major"
    if has_feat:
        return "minor"
    return "patch"


def update_cargo_toml(new_version: str) -> None:
    content = CARGO_TOML_PATH.read_text(encoding="utf-8")
    lines = content.splitlines(keepends=True)
    in_workspace_pkg = False
    replaced = False

    new_lines = []
    for line in lines:
        trimmed = line.strip()
        if trimmed == "[workspace.package]":
            in_workspace_pkg = True
            new_lines.append(line)
            continue
        elif trimmed.startswith("[") and in_workspace_pkg:
            in_workspace_pkg = False

        if in_workspace_pkg and re.match(r'^version\s*=\s*"[^"]+"', trimmed):
            new_lines.append(f'version = "{new_version}"\n')
            replaced = True
        else:
            new_lines.append(line)

    if not replaced:
        raise RuntimeError("Failed to update version in Cargo.toml")

    CARGO_TOML_PATH.write_text("".join(new_lines), encoding="utf-8")


def update_airgap_manifest(new_version: str) -> None:
    if not MANIFEST_PATH.exists():
        print(f"Warning: {MANIFEST_PATH} not found, skipping manifest update.")
        return

    data = json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))
    data["dmz_version"] = new_version
    MANIFEST_PATH.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def update_cargo_lock() -> None:
    print("[+] Running `cargo check --workspace` to synchronize Cargo.lock...")
    subprocess.check_call(["cargo", "check", "--workspace"], cwd=ROOT_DIR)


def update_changelog(new_version: str, commits: list[str]) -> str:
    today = datetime.date.today().isoformat()
    feats = []
    fixes = []
    others = []

    for c in commits:
        if c.startswith("chore(release):"):
            continue
        if re.match(r"^feat(\([^\)]+\))?:", c):
            feats.append(c)
        elif re.match(r"^fix(\([^\)]+\))?:", c):
            fixes.append(c)
        else:
            others.append(c)

    sections = [f"## [v{new_version}] - {today}\n"]
    if feats:
        sections.append("### 🚀 Features")
        for f in feats:
            sections.append(f"- {f}")
        sections.append("")
    if fixes:
        sections.append("### 🐛 Bug Fixes")
        for f in fixes:
            sections.append(f"- {f}")
        sections.append("")
    if others:
        sections.append("### 🔧 Maintenance & Chores")
        for o in others:
            sections.append(f"- {o}")
        sections.append("")

    release_notes = "\n".join(sections).strip()

    if CHANGELOG_PATH.exists():
        old_content = CHANGELOG_PATH.read_text(encoding="utf-8")
        if old_content.startswith("# Changelog"):
            rest = old_content[len("# Changelog") :].lstrip()
            new_content = f"# Changelog\n\n{release_notes}\n\n{rest}"
        else:
            new_content = f"# Changelog\n\n{release_notes}\n\n{old_content}"
    else:
        new_content = f"# Changelog\n\nAll notable changes to the DMZ Platform are documented here.\n\n{release_notes}\n"

    CHANGELOG_PATH.write_text(new_content, encoding="utf-8")
    return release_notes


def main():
    parser = argparse.ArgumentParser(description="DMZ Workspace Release Preparation")
    parser.add_argument("--bump", choices=["auto", "patch", "minor", "major"], default="auto")
    parser.add_argument("--version", help="Explicit target version (e.g. 1.2.4)")
    parser.add_argument("--dry-run", action="store_true", help="Print actions without modifying files")
    args = parser.parse_args()

    current_ver = get_current_workspace_version()
    latest_tag = get_latest_git_tag()
    commits = get_commits_since_tag(latest_tag)

    print(f"Current workspace version: {current_ver}")
    print(f"Latest git tag: {latest_tag or 'None'}")
    print(f"Commits since tag: {len(commits)}")

    if args.version:
        new_ver = args.version
    else:
        if args.bump == "auto":
            if not commits:
                print("No new commits found since last release. Workspace is up-to-date.")
                sys.exit(0)
            bump = detect_bump_type(commits)
            print(f"Detected bump type based on commits: {bump}")
        else:
            bump = args.bump
        new_ver = bump_version(current_ver, bump)

    print(f"Target new version: {new_ver}")

    if args.dry_run:
        print("[Dry Run] Would update Cargo.toml, Cargo.lock, airgap_manifest.json, and CHANGELOG.md")
        print(f"NEW_VERSION={new_ver}")
        return

    update_cargo_toml(new_ver)
    print(f"[+] Updated Cargo.toml to version {new_ver}")

    update_airgap_manifest(new_ver)
    print(f"[+] Updated dmz-core/workspace/airgap_manifest.json to version {new_ver}")

    update_cargo_lock()
    print("[+] Synchronized Cargo.lock")

    update_changelog(new_ver, commits)
    print("[+] Updated CHANGELOG.md")

    # Output for GitHub Actions environment file if available
    gh_output = Path(sys.argv[0]).parent / "GITHUB_OUTPUT"
    import os
    if "GITHUB_OUTPUT" in os.environ:
        with open(os.environ["GITHUB_OUTPUT"], "a") as f:
            f.write(f"new_version={new_ver}\n")
            f.write(f"has_changes=true\n")

    print(f"SUCCESS: Release preparation complete for v{new_ver}")


if __name__ == "__main__":
    main()
