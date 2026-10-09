
### **1. Create GitHub Repository**

1. Log in to [GitHub](https://github.com).
2. Click the **`+`** icon in the top right corner and select **New repository**.
3. Name your repository (e.g., `dmz`), choose visibility (Public or Private), and leave README/gitignore unselected (since you already have a local git project).
4. Click **Create repository**.

### **2. Link Local Project to GitHub**

Run the following commands in your local repository terminal

```bash
git remote add origin https://github.com/rosetta-dmz/dmz.git
git branch -M main
git push -u origin main
```

### **3. Grant Actions Permission**

If GitHub Actions fails to push the release due to permissions:

1. Go to your repository on GitHub -> **Settings** -> **Actions** -> **General**.
2. Scroll down to **Workflow permissions**.
3. Ensure **Read and write permissions** is selected, then click **Save**.

### **4. Tag and Release**

Once linked, run the tagging commands:

```bash
git tag -a v0.1.0 -m "DMZ v0.1.0: Initial Daemonless Sandbox Platform Release"
git push origin v0.1.0
```

This will instantly trigger the GitHub Actions workflow to build and package your cross-platform binaries for Linux (`musl`), macOS, and Windows!