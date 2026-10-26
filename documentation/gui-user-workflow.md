# DMZ GUI Workflow Guide

## Launching the GUI

```bash
cargo run --package dmz-gui
# Or if installed:
dmz-gui
```

---

## Workspace Selection (Open Location)

DMZ uses a shared active workspace concept across all operations:

- **Global Header**: The top header bar displays `Workspace: <path>` alongside a **Browse...** button, visible and accessible from every tab.
- **In-Page Selectors**: The **Air-Gap Manager**, **Resolver**, and **Workspace Sandbox** tabs each feature a dedicated **Workspace directory** field.
- **Synchronization**: Changing the workspace anywhere instantly synchronizes across all tabs.
- **Path Anchoring**: All relative paths (`dmz.lock`, `.dmz/store`, `dist/...`) automatically anchor to the selected workspace directory.

---

## Dependency Scanning & Resolution (Resolver Tab)

1. Select the **Resolver** tab from the left sidebar.
2. Verify your **Workspace directory** (or click **Browse...** to select your codebase).
3. Paths default cleanly relative to your workspace:
   - **Lockfile path**: `dmz.lock` (resolves to `<workspace>/dmz.lock`).
   - **Store cache directory**: `.dmz/store` (resolves to `<workspace>/.dmz/store`).
4. Click **Scan & resolve dmz.lock** to scan manifests (`package.json`, `Cargo.toml`, etc.) and compute closure signatures.
5. Click **Fetch into store (dmz fetch)** to download and authenticate offline packages.

---

## Exporting an Air-Gap Bundle (Export & Save Location)

1. Select the **Air-Gap Manager** tab.
2. Confirm the **Workspace directory** is set to the codebase you wish to export.
3. Configure the **Bundle archive (.tar.zst)** save location:
   - **Default**: `dist/workspace_closure.tar.zst` (anchored inside `<workspace>/dist/`).
   - **Custom Save Location**: Click **Browse...** (the file dialog automatically opens inside your workspace).
   - **Verification**: Check the **Effective archive destination** preview under the inputs to confirm the exact absolute path before exporting.
4. Choose **Package contents**:
   - `Unified (Code + Deps)`: Complete self-contained offline bundle.
   - `Code Only`: Application source code only.
   - `Dependencies Only`: Verified package store only.
5. Adjust the **Zstandard level** slider (default is 3; higher levels reduce bundle size at the cost of build time).
6. Click **Export Bundle** (or one-shot resolution + packaging) to resolve, fetch, and export in a single click, or **Export Archive** if dependencies are already fetched into `.dmz/store`.

---

## Importing & Restoring a Bundle (Import)

1. On the isolated or target machine, launch `dmz-gui` and select **Air-Gap Manager**.
2. Set **Workspace directory** to your destination folder where you want the project unpacked.
3. In **Bundle archive (.tar.zst)**, enter or **Browse...** to select the `.tar.zst` file to import.
4. Toggles:
   - Enable **Install offline dependencies into workspace layouts upon import** to automatically link packages into `node_modules` or `.cargo`.
   - Enable **Cryptographically verify closure hash** for tamper-proof validation.
5. Click **Import & verify bundle**.

---

## Isolated Sandbox Testing (Workspace Sandbox Tab)

1. Select the **Workspace Sandbox** tab.
2. Confirm **Workspace directory** and toggle **Sandbox Process Isolation** or network access.
3. Enter your test command (e.g. `cargo check --offline` or `npm test`) and click **Execute in Sandbox**, or click **Launch isolated terminal shell session** to inspect the workspace in an interactive shell.
