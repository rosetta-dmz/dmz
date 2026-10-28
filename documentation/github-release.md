# DMZ Platform Release & Versioning Guide for Maintainers

Version bumping, manifest synchronization, changelog generation, git tagging, and release publishing are strictly maintainer-initiated.

---

## Maintainer-Initiated Release Options

### Option A: Semi-Automated Release via Pull Request
Use this path when you want GitHub Actions to automatically compute the next Semantic Version, synchronize workspace manifests, and generate categorized release notes from Conventional Commits before you review and approve.

#### Trigger Release Preparation for Maintainers
1. Navigate to GitHub: Actions ➔ Automated Release PR & Tagging.
2. Click Run workflow.
3. Select the version bump level (`auto`, `patch`, `minor`, or `major`).
4. The workflow:
   - Analyzes Conventional Commits since the last release tag.
   - Updates [`Cargo.toml`](../Cargo.toml) (`[workspace.package].version`).
   - Updates [`Cargo.lock`](../Cargo.lock) (synchronized via `cargo check --workspace`).
   - Updates [`dmz-core/workspace/airgap_manifest.json`](../dmz-core/workspace/airgap_manifest.json) (`dmz_version`).
   - Prepend categorized notes to [`CHANGELOG.md`](../CHANGELOG.md).
   - Opens or updates a proposal Pull Request titled `chore(release): DMZ v*` on branch `release/next`.

#### Mandatory Manual Review & Approval
1. Open the pending `release/next` Pull Request on GitHub.
2. Review the manifest diffs and generated release notes in `CHANGELOG.md`.
3. Submit a manual review and Approve the Pull Request.
4. Click Merge pull request into `main`.

#### Publish
Once merged into `main`:
```bash
git checkout main
git pull origin main
git tag -a v* -m "DMZ v*: DMZ Platform Release"
git push origin v*
```

---

### Option B: Direct On-Demand Tag Release

1. Ensure the workspace version in root [`Cargo.toml`](../Cargo.toml) matches your intended release tag.
2. Tag and push directly from your terminal:
```bash
git checkout main
git pull origin main
git tag -a v* -m "DMZ v*: DMZ Platform Release"
git push origin v*
```
