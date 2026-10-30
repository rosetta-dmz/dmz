# DMZ Platform Release & Versioning Guide for Maintainers

Version bumping, manifest synchronization, changelog generation, git tagging, and release publishing are strictly maintainer-initiated and reviewed.

---

### Option A: Automated Release Pull Request via release-plz

For automated version calculation, workspace synchronization, and changelog maintenance based on Conventional Commits, while preserving mandatory manual review and approval before publishing:

1. When pull requests with Conventional Commits (`feat:`, `fix:`, etc.) are merged into `main`, the `release-plz` workflow runs automatically.
2. It analyzes commits since the last release tag and opens (or updates) a single pending Release Pull Request titled `chore: release v*`.
3. The PR automatically:
   - Calculates the next Semantic Version.
   - Updates [`Cargo.toml`](../Cargo.toml) (`[workspace.package].version`).
   - Updates [`Cargo.lock`](../Cargo.lock).
   - Updates [`CHANGELOG.md`](../CHANGELOG.md) with categorized release notes.

#### Mandatory Manual Review & Approval
1. Open the pending `chore: release v*` Pull Request on GitHub.
2. Review the version increment, manifest diffs, and changelog entries.
3. Submit a manual review and Approve the Pull Request.
4. Click Merge pull request into `main`.

#### Automated Binary Build and Release
Once merged into `main`:
1. `release-plz` cuts the Git tag `v*` and creates the GitHub Release notes.
2. The Git tag push automatically triggers the `Release DMZ Platform` workflow (`.github/workflows/release.yml`).
3. Multi-platform binaries (`ubuntu-24.04`, `macos-15`, `windows-2022`) are compiled, packaged (`DMZ.app`, Windows zip, Linux tarball), and attached to the GitHub Release.

---

### Option B: Direct On-Demand Release

When releasing a specific version tag directly without an automated release PR:

#### Via GitHub Actions UI (Workflow Dispatch)
1. Navigate to GitHub: Actions -> Release DMZ Platform.
2. Click Run workflow.
3. Optionally enter the target tag in the Tag to release field (for example, `v1.2.9`).
4. Click Run workflow to start cross-platform builds and asset publishing.

#### Via Git Tag
1. Ensure the workspace version in root [`Cargo.toml`](../Cargo.toml) matches your intended release tag.
2. Tag and push directly from your terminal:
```bash
git checkout main
git pull origin main
git tag -a v* -m "DMZ v*: DMZ Platform Release"
git push origin v*
```
