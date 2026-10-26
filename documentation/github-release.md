# **Link Local Project & Sync with Remote**

```bash
git branch -M main

# Pull any existing remote changes (such as an initial README or license)
git pull origin main

# Create and switch to a release branch
git checkout -b release/v*

# Push the release branch to GitHub to open a Pull Request
git push -u origin release/v*
```

**Note**: Open and merge Pull Request into main on GitHub before proceeding to tag and release.

# **Tag & Release**

```bash
# Switch back to main and pull the latest merged changes
git checkout main
git pull origin main

# Create an annotated tag for the release version
git tag -a v* -m "DMZ v*: DMZ Platform Release"

# Push the tag to trigger the GitHub Actions release workflow
git push origin v*
```

**Note**: Creating the tag (`git tag`) is strictly local to your machine. Pushing the tag (`git push origin v*`) is what triggers the GitHub Actions release workflow ([`.github/workflows/release.yml`](../.github/workflows/release.yml)), which provisions multi-platform runners (`ubuntu-24.04`, `macos-15`, `windows-latest`) to compile CLI/GUI binaries, generate release notes, and publish the release.

```bash
# (Optional) Monitor workflow progress in real-time
gh run watch
```