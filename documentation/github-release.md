# **Link Local Project & Sync with Remote**

```bash
git branch -M main

# Pull any existing remote changes (such as an initial README or license)
git pull origin main

# Create and switch to a release branch
git checkout -b release/v1.0.9

# Push the release branch to GitHub to open a Pull Request
git push -u origin release/v1.0.9
```

**Note**: Open and merge Pull Request into main on GitHub before proceeding to tag and release.

# Tag & Release

```bash
# Switch back to main and pull the latest merged changes
git checkout main
git pull origin main

# Tag and push the release version
git tag -a v1.0.9 -m "DMZ v1.0.9: DMZ Platform Release"
git push origin v1.0.9