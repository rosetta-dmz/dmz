# DMZ Platform Release & Versioning Guide

Starting with **v1.2.4 and beyond**, version bumping, manifest synchronization, changelog generation, git tagging, and release publishing are **completely automated by GitHub Actions**. Developers no longer need to manually modify `Cargo.toml`, `Cargo.lock`, or `airgap_manifest.json`.

---

## 🚀 Fully Automated Release Process (Default)

### 1. Write Conventional Commits
All standard development merges into `main` through Pull Requests. Use [Conventional Commits](https://www.conventionalcommits.org/) to allow CI to determine the next Semantic Version:
- `feat:` or `feat(...):` ➔ Triggers a **minor** release (e.g., `1.2.3` ➔ `1.3.0`)
- `fix:` or `fix(...):` ➔ Triggers a **patch** release (e.g., `1.2.3` ➔ `1.2.4`)
- `BREAKING CHANGE:` or `feat!: ...` ➔ Triggers a **major** release (e.g., `1.2.3` ➔ `2.0.0`)
- `chore:`, `perf:`, `docs:`, `refactor:` ➔ Categorized in changelogs; triggers patch release if needed.

### 2. Automated Release PR (`release/next`)
Whenever commits land on `main`, the [`.github/workflows/release-pr.yml`](../.github/workflows/release-pr.yml) workflow runs:
1. It analyzes commits since the last release tag.
2. It automatically updates:
   - Root [`Cargo.toml`](../Cargo.toml) (`[workspace.package].version`)
   - [`Cargo.lock`](../Cargo.lock) (synchronized via `cargo check --workspace`)
   - [`dmz-core/workspace/airgap_manifest.json`](../dmz-core/workspace/airgap_manifest.json) (`dmz_version`)
   - [`CHANGELOG.md`](../CHANGELOG.md) (categorized by Features, Bug Fixes, and Maintenance)
3. It opens or updates a Pull Request titled `chore(release): DMZ vX.Y.Z` on branch `release/next`.

### 3. One-Click Publishing (Merge the Release PR)
When you are ready to ship the release:
1. Review the open `release/next` Pull Request.
2. Click **Merge pull request** into `main`.
3. The [`.github/workflows/release.yml`](../.github/workflows/release.yml) workflow immediately:
   - Detects the new version in `Cargo.toml`.
   - Automatically cuts and pushes git tag `vX.Y.Z`.
   - Spawns multi-platform matrix runners (`ubuntu-24.04`, `macos-latest`, `windows-latest`).
   - Compiles CLI and GUI binaries (including macOS `.app` bundle, Windows `.zip`, Linux `.tar.gz`).
   - Publishes the GitHub Release with all platform artifacts and generated release notes!

---

## 🛠 Manual & Emergency Release Options

### Option A: GitHub Actions UI (One-Click Dispatch)
If you ever want to force a specific release without waiting for automated commit triggers:
1. Navigate to **Actions** ➔ **Release DMZ Platform** (or **Prepare Release PR**).
2. Click **Run workflow**.
3. Select the desired bump level (`auto`, `patch`, `minor`, `major`) or specify a target tag.

### Option B: Manual Tag Push (Backwards Compatible)
Manual tagging remains fully supported:
```bash
git checkout main
git pull origin main
git tag -a v1.2.4 -m "DMZ v1.2.4: DMZ Platform Release"
git push origin v1.2.4
```
The `validate-tag` check in `release.yml` ensures `[workspace.package].version` in `Cargo.toml` matches the tag before provisioning compilation runners.

---

## ⚙️ Required GitHub Repository Settings

To ensure GitHub Actions can open the automated Release Pull Request:
1. Open the repository on GitHub: **Settings** ➔ **Actions** ➔ **General**.
2. Scroll to **Workflow permissions**.
3. Verify that:
   - **Read and write permissions** is selected.
   - The checkbox **"Allow GitHub Actions to create and approve pull requests"** is checked.