# DMZ Platform Release & Versioning Guide

Starting with **v1.2.6 and beyond**, version bumping, manifest synchronization, changelog generation, git tagging, and release publishing are **strictly maintainer-initiated**. Routine development commits never trigger automated PR drafting or build jobs, and no release or code is ever published without mandatory manual human review and approval on GitHub. This guide is intended for project maintainers.

---

## 🚀 Maintainer-Initiated Release Options

Maintainers can publish releases using either of two fully supported approaches:

### Path 1: Semi-Automated Release via Pull Request (Recommended)
Use this path when you want GitHub Actions to automatically compute the next Semantic Version, synchronize workspace manifests, and generate categorized release notes from Conventional Commits before you review and approve.

#### 1. Trigger Release Preparation
1. Navigate to GitHub: **Actions** ➔ **Automated Release PR & Tagging**.
2. Click **Run workflow**.
3. Select the version bump level (`auto`, `patch`, `minor`, or `major`).
4. The workflow:
   - Analyzes Conventional Commits since the last release tag.
   - Updates [`Cargo.toml`](../Cargo.toml) (`[workspace.package].version`).
   - Updates [`Cargo.lock`](../Cargo.lock) (synchronized via `cargo check --workspace`).
   - Updates [`dmz-core/workspace/airgap_manifest.json`](../dmz-core/workspace/airgap_manifest.json) (`dmz_version`).
   - Prepend categorized notes to [`CHANGELOG.md`](../CHANGELOG.md).
   - Opens or updates a proposal Pull Request titled `chore(release): DMZ vX.Y.Z` on branch `release/next`.

> [!IMPORTANT]
> The workflow **only creates and updates the PR description**. It **never** approves, merges, or publishes code. The PR remains open in GitHub until maintainers manually inspect and approve it.

#### 2. Mandatory Manual Review & Approval
1. Open the pending `release/next` Pull Request on GitHub.
2. Review the manifest diffs and generated release notes in `CHANGELOG.md`.
3. Submit a manual review and **Approve** the Pull Request.
4. Click **Merge pull request** into `main`.

#### 3. Publish the Release
Once merged into `main`, publish the release by pushing the tag or triggering the release workflow:
```bash
git checkout main
git pull origin main
git tag -a v1.2.6 -m "DMZ v1.2.6: DMZ Platform Release"
git push origin v1.2.6
```
*(Alternatively, navigate to **Actions** ➔ **Release DMZ Platform** ➔ **Run workflow**).*

---

### Path 2: Direct On-Demand Tag Release (Bypassing PR)
If you want to cut a release directly without going through the release PR workflow:

1. Ensure the workspace version in root [`Cargo.toml`](../Cargo.toml) matches your intended release tag.
2. Tag and push directly from your terminal:
```bash
git checkout main
git pull origin main
git tag -a v1.2.6 -m "DMZ v1.2.6: DMZ Platform Release"
git push origin v1.2.6
```

---

## 📦 What the Release Pipeline Executes

Whenever a release tag (`v*`) is pushed or manually dispatched, [`.github/workflows/release.yml`](../.github/workflows/release.yml) runs:

1. **Tag Validation:** Verifies that the git tag (`vX.Y.Z`) matches `[workspace.package].version` in `Cargo.toml`.
2. **Multi-Platform CLI Build:** Provisions matrix runners (`ubuntu-24.04`, `macos-latest`, `windows-latest`) to compile:
   - Linux: `x86_64-unknown-linux-musl`
   - macOS: `aarch64-apple-darwin`
   - Windows: `x86_64-pc-windows-msvc`
3. **Multi-Platform GUI Packaging:**
   - Linux: `DMZ-GUI-Linux.tar.gz`
   - macOS: `DMZ-GUI-macOS.zip` (standalone `.app` bundle, AppIcon, Info.plist, codesigned)
   - Windows: `DMZ-GUI-Windows.zip`
4. **Publishing:** Publishes the official GitHub Release with all 6 platform assets attached and auto-generated release notes.

---

## 🛡 Mandatory Branch Protection & Approval Rules

To enforce that **no code or release can ever be merged without human review**, configure GitHub Branch Protection on `main`:

1. Open repository **Settings** ➔ **Branches** (or **Rulesets**).
2. Under **Branch protection rules**, add or edit a rule for `main`:
   - ✅ **Require a pull request before merging**
   - ✅ **Require approvals** (set required approvals to at least `1`)
   - ✅ **Dismiss stale pull request approvals when new commits are pushed**
   - ✅ **Require status checks to pass before merging** (e.g. `Continuous Integration`)
   - ✅ **Do not allow bypassing the above settings**
3. Save changes. This guarantees that every PR—including the automated `release/next` PR—must be explicitly reviewed and approved by an authorized human maintainer before GitHub permits merging.

---

## ⚙️ Required GitHub Actions Permissions

To allow GitHub Actions to open the proposed Release Pull Request when manually dispatched:
1. Open the repository on GitHub: **Settings** ➔ **Actions** ➔ **General**.
2. Scroll to **Workflow permissions**.
3. Verify that:
   - **Read and write permissions** is selected.
   - The checkbox **"Allow GitHub Actions to create and approve pull requests"** is checked.

> [!NOTE]
> **Why is GitHub's setting labeled "create and approve"?**  
> GitHub bundles REST API permissions for PR creation and PR review into a single permission toggle. Enabling this checkbox is required so the bot can call `gh pr create` when you click **Run workflow**. The DMZ release workflow **never executes approval commands** (`gh pr review --approve`), and GitHub branch protection rules strictly prevent workflows from bypassing required human reviews.
