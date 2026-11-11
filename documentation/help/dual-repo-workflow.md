# Dynamic Multi-Repo & Companion Service Workflow in DMZ

> **Guide for orchestrating, sandboxing, and air-gap packaging companion services (Python, Rust, Node, Go, Tauri, etc.) across multiple repositories without rigid hierarchies.**

---

## Overview & Architecture

Modern desktop and full-stack applications often pair two or more cooperating codebases, such as a desktop client (Tauri / Electron / Node) and a companion backend (Python AI worker, Rust/C++ computation engine, or IPC daemon).

DMZ treats all codebases as first-class, dynamic workspace members. Each repository maintains its own isolated toolchains and offline package store, verified with zero host dependencies, and bridged over **daemonless local IPC**:

```
┌────────────────────────────────────────────────────────┐
│                      DMZ Project                       │
│                                                        │
│  ┌──────────────────────┐    ┌──────────────────────┐  │
│  │ Workspace Member A   │    │ Workspace Member B   │  │
│  │ (e.g. Tauri / Node)  │    │ (e.g. Python / Rust) │  │
│  └──────────┬───────────┘    └──────────┬───────────┘  │
│             │ R/W                       │ R/W          │
│             ▼                           ▼              │
│       Sandbox A                   Sandbox B            │
│    (deny network*)             (deny network*)         │
│             │                           │              │
│             └───► /tmp/dmz_[hash]/ ◄────┘              │
│                     ipc.sock                           │
│              (Pure Local Unix Socket)                  │
└────────────────────────────────────────────────────────┘
```

### Core Guarantees:
* **Zero Network Exposure (`deny network*`):** Communication happens strictly over filesystem-based Unix Domain Sockets (macOS/Linux) or Named Pipes (Windows). No TCP ports, loopback proxies, or open network attack surfaces.
* **POSIX `sun_path` Safe (<104 chars):** Sockets are anchored to short-hashed paths (e.g., `/tmp/dmz_[hash]_[id].sock`), preventing POSIX buffer overflow crashes on deep directories.
* **Fail-Fast KEP-753 Lifecycle Sequencing:** Companion services boot first $\rightarrow$ passes active socket readiness probe $\rightarrow$ client app is launched with `DMZ_SIDECAR_SOCK`. If a service crashes on boot, the probe aborts in <20ms and prints stderr instead of hanging for 10 seconds.
* **Process Group Isolation (`setpgid` / Job Objects):** Every member runs in a dedicated process group. When stopping or on `Ctrl+C`, the entire process tree (including compilers, node child processes, and watchers) is reaped with zero orphaned zombie processes.
* **Virtual Runfiles Tree:** Sibling paths (`../backend`) are mapped internally to canonical roots (`workspaces/<id>`), preventing CI and cross-machine directory drift.

---

## Workspace Declaration (`dmz-workspace.toml`)

To link two or more repositories, create a `dmz-workspace.toml` in your anchor project root or umbrella folder:

```toml
name = "desktop-ai-suite"
version = "1.0.0"

# First workspace member (e.g. frontend / desktop UI)
[[members]]
id = "client"
path = "."
role = "primary"
ecosystem = "node"

# Second workspace member (e.g. companion AI engine)
[[members]]
id = "backend"
path = "../ai-engine"
role = "sidecar"
ecosystem = "python"
is_static_binary = false
exec_command = "python3"
exec_args = ["server.py"]

[ipc]
socket_id = "backend"
env_var = "DMZ_SIDECAR_SOCK"
readiness_timeout_ms = 10000
drain_timeout_ms = 3000
```

> **Dynamic Roles:** `role` defaults to `"member"`. You can also designate `"primary"`, `"sidecar"`, or `"service"`. Member IDs are semantic identifiers (e.g. `client`, `backend`, `auth-service`) that define clean multi-root folder titles and log prefixes.

---

## CLI Developer Workflow

### Step 1: Scan & Resolve Heterogeneous Dependencies
Resolve manifests across all workspace members into a unified, namespaced `dmz.lock`:

```bash
# Automatically discovers dmz-workspace.toml
dmz resolve

# Or pass ad-hoc workspaces on the command line:
dmz resolve --workspace ./client --sidecar ../ai-engine
```

#### Dual-Stack Tauri Auto-Detection:
In Tauri projects, DMZ automatically scans both root `package.json` and nested `src-tauri/Cargo.toml`. It locks:
- All frontend npm dependencies and the companion native CLI binary package (`@tauri-apps/cli-<platform>`).
- All Tauri Rust crates (`tauri`, `tauri-build`, `serde`, etc.) into `dmz.lock`.

### Step 2: Fetch and Cache Locked Closures
Download and cache offline dependencies for all members into `.dmz/store`:

```bash
dmz fetch
```

### Step 3: Run Coordinated Execution (`dmz run`)
Launch the workspace under supervisor orchestration:

```bash
dmz run --workspace .
```

`dmz run` coordinates the full lifecycle:
1. Spawns companion services first in dedicated process groups (`setpgid`).
2. Streams non-blocking stdout/stderr multiplexed with color-coded tags (`[backend]` in magenta, `[client]` in cyan).
3. Verifies socket readiness with fail-fast premature exit detection.
4. Auto-detects the client launch command (`npm run tauri dev`, `npm run dev`, `cargo run`, or configured command) and injects `DMZ_SIDECAR_SOCK`.
5. On `Ctrl+C` (SIGINT), executes graceful reverse drain and terminates all descendant process trees cleanly.

Inside your client application code, simply connect to the socket path provided by the environment variable:

```typescript
// Node / Electron / Tauri client example:
import net from 'net';

const socketPath = process.env.DMZ_SIDECAR_SOCK || '/tmp/fallback.sock';
const client = net.createConnection({ path: socketPath }, () => {
    console.log('Connected to local companion IPC bridge!');
    client.write(JSON.stringify({ type: 'Ping' }));
});
```

### Step 4: Air-Gap Closure Export & Distribution
Package all member source trees, offline package stores, documentation, dist assets, and compiler toolchains into a single verifiable `.tar.zst` umbrella bundle:

```bash
dmz bundle --with-toolchain --output dist/project_closure.tar.zst
```

The exported umbrella archive preserves original repository directory names and generates:
- `<member-a>/`: Complete source tree for workspace member A.
- `<member-b>/`: Complete source tree for workspace member B.
- `documentation/`: Bundled guides and architecture specifications.
- `dist/`: Pre-built release deliverables (if present).
- `dmz-workspace.toml`: Multi-workspace orchestrator manifest.
- `<project>.code-workspace`: VS Code Multi-Root Workspace file.
- `README.md`: Air-gap quickstart runbook.

### Step 5: Unpack & Open on Clean Air-Gapped Machine
On target air-gapped systems with zero pre-installed runtimes:

```bash
# Unpack the umbrella bundle
dmz import --archive dist/project_closure.tar.zst --workspace ./project

# Enter the hermetic environment
cd project

# Open the multi-root workspace in VS Code or Cursor:
code my-desktop-suite.code-workspace

# Or run directly via the DMZ supervisor:
dmz run --workspace .
```

#### Automatic Dependency Hydration:
Upon import, DMZ automatically hydrates:
- **Tauri / npm:** Generates `node_modules/.bin/tauri` and pre-configures terminal PATH so `@tauri-apps/cli` is immediately available.
- **Cargo:** Automatically configures `.cargo/config.toml` vendored source replacement in both root and `src-tauri/.cargo/config.toml` pointing to `.dmz/store` for offline builds (`cargo build --offline`).
- **Python (Smart Detection):** Configures folder-scoped `.code-workspace` settings. If `.venv` exists, points to `${workspaceFolder}/.venv/bin/python`. If bundled toolchain exists, points to `${workspaceFolder}/../.dmz/toolchains/python/bin/python3`. If no toolchain or venv exists, omits broken keys to eliminate `"Invalid Python interpreter"` error popups.

---

## GUI Developer Workflow

For developers using the visual desktop interface (`dmz-gui`), the multi-repo workflow spans three dedicated pages: **Dependency Resolver**, **Air-Gap Manager**, and **Supervisor IPC**.

### Step 1: Configure Multi-Repo in Dependency Resolver
1. Open the **Dependency Resolver** page.
2. In **Workspace Mode**, toggle to **Multi-Repo / Companion**.
3. Select **Workspace 1 (Anchor Root)** and **Workspace 2 (Companion)**.
4. **Member ID & Tag Settings:**
   * **Member ID:** Auto-populates with the companion folder's name (e.g. `backend`, `api-server`, `worker`).
   * **Member Role / Tag:** Set to `member`, `sidecar`, or `service`.
5. **Click "Save as dmz-workspace.toml":**
   * Persists `dmz-workspace.toml` directly into your anchor workspace root.

### Step 2: Scan & Resolve Heterogeneous Dependencies
1. Click **Scan & Resolve**.
2. **What DMZ performs automatically:**
   * Scans package manifests across all codebases simultaneously (including `package.json`, `src-tauri/Cargo.toml`, and `pyproject.toml`).
   * Executes dynamic ABI diagnostics on companion native binaries.
   * Generates a unified, namespaced `dmz.lock` where each package is mapped to its `member_id`.

### Step 3: Air-Gap Export in Air-Gap Manager
1. Switch to the **Air-Gap Manager** page.
2. In the **Bundle & Archive Paths** card:
   * **Workspace Directory:** Points to your anchor workspace.
   * **Bundle Archive (.tar.zst):** Choose destination path (e.g., `dist/project_airgap.tar.zst`).
3. Click **Bundle (Fetch)**:
   * Fetches dependencies for all members into `.dmz/store` (including `@tauri-apps/cli` companion binary packages).
   * Bundles closures, toolchains, source trees, and manifests into a self-contained archive with a `.sha256` integrity sidecar.

### Step 4: Import & Hydrate in Air-Gap Manager (Air-Gapped Host)
1. On the air-gapped machine, open the **Air-Gap Manager** page.
2. Under **Import Air-Gap Archive**, click **Open Archive...** to select your `.tar.zst` bundle.
3. Set your target **Workspace Directory**.
4. Keep **"Install offline dependencies into workspace"** and **"Cryptographically verify closure hash"** checked.
5. Click **Import & Verify Archive**:
   * **Integrity Verification:** Validates the archive signature and packages against the closure manifest.
   * **Source & Store Unpack:** Restores all member source trees, `.dmz/store`, and offline toolchains.
   * **Automatic Hydration:** Auto-configures offline `.cargo/config.toml` vendoring (root and `src-tauri`), links native CLI binaries (`node_modules/.bin/tauri`), and generates `<project>.code-workspace` with smart Python interpreter paths.
   * **Ready to Work:** Open `<project>.code-workspace` in VS Code / Cursor or proceed to supervised execution.

### Step 5: Live Supervised Execution in Supervisor IPC
1. Navigate to the **Supervisor IPC** page.
2. In the **Target Umbrella Workspace** card:
   * Confirm or browse to your imported umbrella workspace root directory.
   * Verify the green indicator showing `dmz-workspace.toml` loaded with your Primary and Sidecar member IDs.
3. (Optional) In the telemetry grid, adjust the **Readiness Gate Timeout** slider:
   * Keep default (10,000 ms) for Unix domain socket sidecars.
   * Set to `0 ms` to skip the socket probe if your companion service uses HTTP/REST or another protocol.
4. Click **Launch Workspace Pair**:
   * Wires directly into real `SidecarOrchestrator` execution on background Tokio runtime.
   * Auto-detects member run commands and injects hermetic `.dmz/toolchains` into PATH while stripping `PYTHONHOME`.
   * **Stage Highlights:** Live-transitions across `Clean Socket` $\rightarrow$ `Spawning Services` $\rightarrow$ `Readiness Probe` $\rightarrow$ `Services Ready` $\rightarrow$ `Active Bridge`.
   * Real socket path, byte length, and error telemetry are displayed live.
   * If a launch or readiness issue occurs, a red alert banner displays the exact error message with a **Dismiss & Reset to Standby** button to reset the state cleanly.
5. Click **Stop Pair (Graceful Drain)**:
   * Triggers real reverse graceful drain: stops client application $\rightarrow$ drains active IPC $\rightarrow$ terminates companion services and unlinks socket.

---

## Security & Best Practices

| Recommendation | Reason |
| :--- | :--- |
| **Rely on `dmz run` for Unified Local Dev** | Automatically coordinates process startup order, verifies socket readiness, multiplexes logs, and guarantees zero orphaned zombie processes on exit. |
| **Prefer Static Companion Binaries (`musl`)** | When compiling companion backends in Rust or Go, compile against `musl` (`x86_64-unknown-linux-musl`) to eliminate host dynamic library (`glibc`) mismatches on air-gapped target machines. |
| **Never Use TCP Loopback for Local IPC** | Loopback networking (`127.0.0.1`) opens sandbox attack surfaces and risks port collisions. Always use Unix domain sockets or Windows Named Pipes. |
| **Rely on `DMZ_SIDECAR_SOCK`** | Read `DMZ_SIDECAR_SOCK` to ensure compliance with POSIX safe path length guarantees (<104 bytes) and automated cleanup. |
| **Supervisor IPC is Optional** | You can always build, test, and run workspaces independently in separate terminal windows or directly via VS Code / Cursor without the supervisor running. |
