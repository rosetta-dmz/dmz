# **Link Local Project & Sync with Remote**

```bash
git branch -M main

# Pull any existing remote changes (such as an initial README or license)
git pull origin main

# Create and switch to a release branch
git checkout -b release/v*

# Bump [workspace.package].version in Cargo.toml to match target release version, then commit
git commit -am "chore: bump version to *"

# Push the release branch to GitHub to open a Pull Request
git push -u origin release/v*
```

**Note**: Opening the Pull Request triggers automated CI testing ([`.github/workflows/ci.yml`](../.github/workflows/ci.yml)) across Ubuntu, macOS, and Windows. Merge the Pull Request into main before proceeding to tag and release.

# **Tag & Release**

```bash
# Switch back to main and pull the latest merged changes
git checkout main
git pull origin main

# Create an annotated tag for the release version (must match Cargo.toml version)
git tag -a v* -m "DMZ v*: DMZ Platform Release"

# Push the tag to trigger the GitHub Actions release workflow
git push origin v*
```

**Note**: Pushing the tag (`git push origin v*`) triggers the release workflow ([`.github/workflows/release.yml`](../.github/workflows/release.yml)). The workflow's `validate-tag` gate automatically verifies that the tag matches `Cargo.toml` before provisioning multi-platform runners (`ubuntu-24.04`, `macos-15`, `windows-latest`) to compile CLI/GUI binaries, generate release notes, and publish the release.

```bash
# (Optional) Monitor workflow progress in real-time
gh run watch
```