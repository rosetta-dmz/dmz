# Dual-Repo Post-Export Import & Execution Guide

> **Step-by-step guide for transferring, importing, verifying, and running a dual-repo (multi-workspace) DMZ bundle after a successful export.**

---

## Overview

After a successful dual-repo export, you have a `.tar.zst` umbrella bundle (for example `dist/workspace_closure.tar.zst` from `dmz bundle` or `dist/airgap_bundle.tar.zst` from `dmz export`) and its `.sha256` sidecar file (`<archive>.sha256`).

Inside a multi-workspace bundle, DMZ packages:
- `workspaces/<primary>/` and `workspaces/<sidecar>/`: Source trees for each repository in isolated namespaces.
- `dmz-workspace.toml`: Normalized manifest with member `path` values pointing to `workspaces/<folder>`.
- `<project>.code-workspace`: VS Code / Cursor multi-root workspace configuration.
- `dmz.lock` and `airgap_manifest.json`: Dependency closure lockfile and archive metadata.
- `store/` and optionally `.dmz/toolchains/` and `workspace.bundle` (when exported with `--with-toolchain` / `--with-git-history`).
- `README.md`: Generated quickstart runbook at the umbrella root.

---

## Step 1: Transfer & Import the Archive on the Target Host

Copy both the `.tar.zst` archive and its `<archive>.sha256` sidecar (if generated) to the target machine. When the `.sha256` sidecar sits alongside the archive, DMZ automatically verifies physical transport integrity against it before unpacking.

### Option A: CLI (`dmz import`)

```bash
dmz import --archive dist/project_closure.tar.zst --to ~/workspaces/project
```

> **macOS Tip:** Prefer a neutral destination directory such as `~/workspaces/project` rather than macOS TCC-protected folders (`~/Documents`, `~/Desktop`, `~/Downloads`) so `dmz` and sandboxed processes don't hit `Operation not permitted (os error 1)` prompts.

Key flags:
- `--to` / `-t` (alias `--workspace`): Target umbrella directory where the project is unpacked (defaults to `workspace`).
- `--include-dev`: Include development dependencies during post-import toolchain validation (use this if the bundle was exported with `--include-dev`).
- `--skip-verify`: Skip closure hash verification *(not recommended)*.
- `--skip-install`: Skip automatic offline dependency installation into project layouts.
- `--skip-ecosystem-verify`: Skip the automatic post-import sandboxed ecosystem check.
- `--no-sandbox`: Run post-import ecosystem verification unsandboxed on the host *(not recommended)*.

When `dmz import` completes, it prints a **Next steps** block tailored to the imported workspace (`dmz-workspace.toml` present vs. single-repo):
```text
Next steps (dual-repo workspace):
  cd ~/workspaces/project
  dmz run --workspace .   # run from your host shell (do not run inside `dmz shell`)
  code <project-name>.code-workspace
```

### Option B: GUI (`Air-Gap Manager`)

1. Launch `dmz-gui` and open the **Air-Gap Manager** tab.
2. Set **Workspace directory** to the target destination folder.
3. Under **Import Air-Gap Archive**, click **Open Archive...** and select the `.tar.zst` bundle.
4. Keep **Install offline dependencies into workspace** and **Cryptographically verify closure hash** checked (and check **Include dev dependencies** if the bundle was exported with dev dependencies).
5. Click **Import & Verify Archive**.

---

## Step 2: What DMZ Automates During Import

During `dmz import` (or **Import & Verify Archive** in the GUI), DMZ performs the following steps automatically:

1. **Transit & Closure Verification**: Validates the `.sha256` sidecar (when present) and verifies `dmz.lock` against its `closure_signature`, creating a pre-import backup snapshot for rollback on failure.
2. **Safe Unpack & Git Restoration**: Extracts the umbrella layout with path-traversal and decompression quota checks, and restores `workspace.bundle` Git history if present.
3. **Pruning & Toolchain Setup**: Prunes store files not referenced by `dmz.lock`, removes unrequested `.dmz/toolchains/<ecosystem>` directories, generates `.dmz/env.sh` / `.dmz/env.bat`, and checks host baseline prerequisites (e.g., macOS Apple CLT/Xcode).
4. **Offline Dependency Installation & Hydration**:
   - Materializes locked packages from `.dmz/store` into project layouts.
   - Links `.dmz`, `dmz.lock`, and `dmz-workspace.toml` into each member directory.
   - Writes `.cargo/config.toml` (and `src-tauri/.cargo/config.toml` when present) pointing to `.cargo/vendor` or the local store for `--offline` builds.
   - Hydrates `node_modules/.bin/*` CLI shims (such as `tauri`).
   - Hydrates member `.venv` + `.vscode/settings.json` and sanitizes `<project>.code-workspace` Python interpreter paths so dangling source-machine paths do not cause IDE errors.
5. **Automated Ecosystem Verification (CLI only)**: `dmz import` automatically discovers active ecosystems across all member roots and runs each ecosystem's native verification command inside a zero-network sandbox (unless `--skip-install` or `--skip-ecosystem-verify` is passed).
   > **Note:** In `dmz-gui`, **Import & Verify Archive** runs the core import, verification, and hydration pipeline without automatically executing the sandboxed ecosystem verification commands. Run `dmz ecosystem verify --workspace ./project` or use the **Workspace Sandbox** tab afterward if you want to run those checks manually.

---

## Step 3: Next Steps After Import Succeeds

Once import completes in `./project`, you can launch the pair, open the multi-root workspace in an IDE, or run tools individually offline.

### A. Launch Both Repos Under Supervised IPC (KEP-753)

The DMZ supervisor boots companion sidecars first, checks IPC socket readiness (`DMZ_SIDECAR_SOCK`), then starts the primary application and performs a reverse graceful drain on shutdown.

#### CLI (`dmz run`)
Run `dmz run` directly from your **host shell** (never nested inside `dmz shell` or `dmz sandbox`, which sets `DMZ_SANDBOX=1`; `dmz run` manages per-member isolation itself):
```bash
cd ./project
dmz run --workspace .
```
Press `Ctrl+C` (`SIGINT`) to trigger a clean reverse graceful drain.

#### GUI (`Supervisor IPC`)
1. Switch to the **Supervisor IPC** tab.
2. Confirm **Target Umbrella Workspace** points to your imported umbrella root (a green indicator confirms `dmz-workspace.toml` is loaded with your Primary and Sidecar member IDs).
3. Adjust **Readiness Gate Timeout** if needed (default `10000 ms` for Unix domain socket sidecars; set to `0 ms` to skip the socket probe if the sidecar uses HTTP/REST or another protocol) and **Teardown Drain Window** (`500–10000 ms`).
4. Click **Launch Workspace Pair** (transitions through `Clean Socket` → `Spawning Services` → `Readiness Probe` → `Services Ready` → `Active Bridge`).
5. Click **Stop Pair (Graceful Drain)** when finished.

### B. Open in VS Code / Cursor

Open the generated `.code-workspace` file at the umbrella root:
```bash
code ./project/<project-name>.code-workspace
```
This opens all member directories under `workspaces/` side-by-side with `rust-analyzer.linkedProjects`, `node_modules/.bin` and `.dmz/toolchains/{node,rust,python}/bin` added to the integrated terminal `PATH`, and validated per-member Python interpreter settings.

### C. Run Tools, Builds, and Tests Offline

You can also work directly inside the imported workspace using any of the offline execution modes:

- **Re-run ecosystem verification**:
  ```bash
  dmz ecosystem verify --workspace ./project
  ```
- **Load bundled toolchains into your current terminal**:
  ```bash
  # macOS / Linux (use .dmz\env.bat on Windows)
  source ./project/.dmz/env.sh
  cargo build --offline
  ```
- **Interactive sandboxed shell**:
  ```bash
  dmz shell --workspace ./project
  # or run a single command and exit:
  dmz shell --workspace ./project -c "<command>"
  ```
- **Single isolated sandbox command** (or use the GUI **Workspace Sandbox** tab):
  ```bash
  dmz sandbox --workspace ./project -- cargo test --offline
  ```

---

## Related Guides

- [Dynamic Multi-Repo & Companion Service Workflow](./dual-repo-workflow.md) — Workspace setup (`dmz-workspace.toml`), resolution, export, and IPC architecture.
- [Running Tools After Air-Gap Import](../air-gap-manager/post-import-execution.md) — Using `.dmz/env.sh`, `dmz shell`, and `dmz sandbox`.
- [CLI Walkthrough](../cli/cli-walkthrough.md) & [GUI Workflow Guide](../gui/gui-user-workflow.md) — Command and UI references.
