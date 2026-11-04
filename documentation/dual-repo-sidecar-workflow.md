# Dual-Repo & Sidecar Workflow in DMZ

> **Guide for orchestrating, sandboxing, and air-gap packaging companion services (Python, Rust, Node, Go) alongside primary applications.**

---

## Overview & Architecture

Modern desktop and full-stack applications often pair a primary frontend/app (e.g. desktop GUI, CLI, or web client) with an independent companion **sidecar** service (e.g. local Python AI worker, Rust/C++ computation backend, or IPC engine).

In unconstrained host environments, these multi-repo projects rely on system-installed dependencies and host networking. In **DMZ**, each repository is hermetically sandboxed, verified with zero host dependencies, and bridged over **daemonless local IPC**:

```
┌────────────────────────────────────────────────────────┐
│                      DMZ Project                       │
│                                                        │
│  ┌──────────────────────┐    ┌──────────────────────┐  │
│  │ Primary Workspace    │    │ Sidecar Workspace    │  │
│  │ (e.g. Python / Rust) │    │ (e.g. Node / GUI)    │  │
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
* **KEP-753 Lifecycle Sequencing:** Sidecar boots first $\rightarrow$ passes active socket readiness probe $\rightarrow$ primary app is launched with `DMZ_SIDECAR_SOCK`. On shutdown, the primary exits first before the sidecar drains and terminates.
* **Virtual Runfiles Tree:** Sibling paths (`../sidecar`) are mapped internally to canonical roots (`workspaces/primary` and `workspaces/sidecars/<id>`), preventing CI and cross-machine directory drift.

---

## Workspace Declaration (`dmz-workspace.toml`)

To link two repositories, create a `dmz-workspace.toml` in your primary project root or umbrella folder:

```toml
name = "desktop-ai-suite"
version = "1.0.0"

# Primary application member
[[members]]
id = "desktop-ui"
path = "."
role = "primary"
ecosystem = "node"

# Companion sidecar member (sibling directory or subfolder)
[[members]]
id = "ai-engine"
path = "../ai-engine"
role = "sidecar"
ecosystem = "python"
is_static_binary = false
exec_command = "python3"
exec_args = ["server.py"]

[ipc]
socket_id = "ai-engine"
env_var = "DMZ_SIDECAR_SOCK"
readiness_timeout_ms = 10000
drain_timeout_ms = 3000
```

---

## CLI Developer Workflow

### Step 1: Scan & Resolve Heterogeneous Dependencies
Resolve both ecosystems into a unified, namespaced `dmz.lock`:

```bash
# Automatically discovers dmz-workspace.toml
dmz resolve

# Or pass ad-hoc workspaces on the command line:
dmz resolve --workspace ./desktop-ui --sidecar ../ai-engine
```

### Step 2: Fetch and Cache Locked Closures
Download and cache dependencies for both stacks into `.dmz/store`:

```bash
dmz fetch
```

### Step 3: Run Sandboxed with Supervisor Orchestration
Launch the isolated environment. `dmz-supervisor` enforces KEP-753 startup sequencing, spawns the sidecar, verifies socket readiness, and exposes `DMZ_SIDECAR_SOCK` to the primary app:

```bash
# Starts both sandboxes bridged via local socket under complete network denial
dmz shell
```

Inside your primary application code, simply connect to the socket path provided by the environment variable:

```typescript
// Node / Electron / Tauri client example:
import net from 'net';

const socketPath = process.env.DMZ_SIDECAR_SOCK || '/tmp/fallback.sock';
const client = net.createConnection({ path: socketPath }, () => {
    console.log('Connected to local sidecar IPC bridge!');
    client.write(JSON.stringify({ type: 'Ping' }));
});
```

### Step 4: Air-Gap Closure Export & Distribution
Package both source trees, their offline package stores, documentation, dist assets, and compiler toolchains into a single verifiable `.tar.zst` umbrella bundle:

```bash
dmz bundle --with-toolchain --output dist/project_closure.tar.zst
```

The exported umbrella archive preserves original repository directory names and generates:
- `<primary-folder-name>/`: Complete primary application source tree.
- `<secondary-folder-name>/`: Complete sidecar repository tree.
- `documentation/`: Bundled guides and architecture specifications.
- `dist/`: Pre-built release deliverables (if present).
- `dmz-workspace.toml`: Normalized multi-workspace orchestrator manifest.
- `<project>.code-workspace`: VS Code Multi-Root Workspace file.
- `README.md`: Air-gap quickstart runbook.

### Step 5: Unpack & Open on Clean Air-Gapped Machine
On target air-gapped systems with zero pre-installed runtimes:

```bash
# Unpack the umbrella bundle
dmz import --archive dist/project_closure.tar.zst --workspace ./project

# Enter the hermetic environment and launch with supervisor orchestration
cd project

# Open the multi-root workspace in VS Code or Cursor:
code my-desktop-suite.code-workspace

# Or run directly via the DMZ supervisor:
dmz run --workspace .
```

#### Automatic Dependency Hydration:
Upon import, DMZ automatically hydrates:
- **Tauri / npm:** Generates `node_modules/.bin/tauri` and pre-configures terminal PATH so `@tauri-apps/cli` is immediately available.
- **Cargo:** Automatically configures `.cargo/config.toml` vendored source replacement pointing to `.dmz/store` for offline builds (`cargo build --offline`).
- **Python:** Isolates Python interpreter paths (`python.defaultInterpreterPath`) strictly to the sidecar subproject inside `.code-workspace`, eliminating global "Python 3 interpreter error" popups.

---

## GUI Developer Workflow

For developers using the visual interface (`dmz-gui`), the dual-repo and sidecar workflow spans three dedicated pages: **Resolver**, **Air-Gap Manager**, and **Supervisor IPC**.

```
┌────────────────────────────────────────────────────────────────────────┐
│  Workspace Mode: [ Single Workspace ]  (•) Dual Repo / Sidecar         │
│                                                                        │
│  Primary Workspace Directory:                                          │
│  [ /Users/.../desktop-ui                        ] [ Browse... ]        │
│  Ecosystem: Node.js (package.json)                                     │
│                                                                        │
│  Sidecar Workspace:                                                    │
│  [ /Users/.../ai-engine                         ] [ Browse... ]        │
│  Ecosystem: Python (pyproject.toml)                                    │
│                                                                        │
│  Sidecar ID:   [ sidecar            ]   Role: [ sidecar              ] │
│                                                                        │
│  [ Save as dmz-workspace.toml ]                                        │
│  [ Scan & Resolve             ]                                        │
└────────────────────────────────────────────────────────────────────────┘
```

### Step 1: Configure Dual Repositories in Dependency Resolver
1. Open the **Dependency Resolver** page.
2. In **Workspace Mode**, toggle to **Dual Repo / Sidecar**.
3. Select the **Primary Workspace Directory** (e.g. desktop UI or frontend client) and the **Sidecar Workspace** (e.g. Python backend, Tauri core, or Rust IPC service).
4. **Sidecar ID & Role Settings:**
   * **Sidecar ID** (defaults to `"sidecar"`):
     * *Leave as default:* For standard setups with one companion sidecar.
     * *Edit:* If you prefer a semantic identifier (e.g., `ai-engine`, `backend`, `worker`). This tags all resolved packages in `dmz.lock`, defines the member in `dmz-workspace.toml`, and names the safe socket (`dmz_<hash>_<id>.sock`).
   * **Sidecar Role** (defaults to `"sidecar"`):
     * *Leave as `"sidecar"`:* For typical companion applications managed directly by the primary app.
     * *Change to `"service"`:* If the process is a decoupled standalone service (e.g. background database or message queue).
5. **Click "Save as dmz-workspace.toml":**
   * Persists the project configuration (`dmz-workspace.toml`) directly into your primary workspace root.
   * **Why:** It binds the two repositories together permanently. All subsequent CLI commands (`dmz shell`, `dmz bundle`, `dmz run`) and future GUI launches will automatically recognize both workspaces without requiring manual folder selection.

### Step 2: Scan & Resolve Heterogeneous Dependencies
1. Click **Scan & Resolve**.
2. **What DMZ performs automatically:**
   * Scans package manifests across **both** codebases simultaneously (e.g., `package.json` + `pyproject.toml` or `Cargo.toml`).
   * Executes dynamic ABI diagnostics on the sidecar (inspecting `.dylib`, `.so`, `.node`, and external glibc symbols).
   * Generates a unified, namespaced [`dmz.lock`] in the primary workspace where each dependency is mapped to its `member_id` (`"primary"` vs. your sidecar ID).

### Step 3: Air-Gap Export in Air-Gap Manager
1. Switch to the **Air-Gap Manager** page.
2. In the **Bundle & Archive Paths** card:
   * **Workspace Directory:** Points to your primary workspace (where [`dmz.lock`] and [`dmz-workspace.toml`] reside).
   * **Bundle Archive (.tar.zst):** Choose your destination file path (e.g., `dist/project_airgap.tar.zst`).
   * *Notice:* You do **not** need to re-specify the sidecar path here. The primary workspace anchors the entire project because the unified lockfile and project manifest already include both workspaces.
3. Configure optional settings:
   * **Package Contents:** Select *Complete Bundle* (source code + dependencies), *Codebase Only*, or *Dependencies Only*.
   * **Package compiler toolchains into bundle:** Check to hermetically vendor language runtimes (e.g., Node + Python sysroots) for air-gapped targets.
4. Click **Export Bundle**:
   * DMZ automatically fetches dependencies for **both** primary and sidecar workspaces into `.dmz/store`.
   * Packages the unified closure, compiler toolchains, source trees, and manifests into a single self-contained `.tar.zst` file alongside a cryptographic `.sha256` integrity sidecar.

> [!TIP]
> **Source Tree Colocation Note:**
> When wanting both source trees bundled physically inside the same archive, placing them under a shared project parent directory or subfolder makes the export completely self-contained. For sibling repositories (e.g., `../sidecar`), all sidecar dependencies, toolchain sysroots, and IPC contracts are fully preserved in the archive via the unified `dmz.lock`.

### Step 4: Supervised Execution & Telemetry in Supervisor IPC
1. Navigate to the **Supervisor IPC** page.
2. Click **Launch Supervised Pair** to test local orchestration under strict KEP-753 protocol:
   * **Cleaning Stale Socket:** Unlinks leftover dead socket files from previous unclean terminations.
   * **Spawning Sidecar:** Launches the sidecar process in an isolated sandbox (`deny network*`).
   * **Readiness Probe:** Probes socket handshake readiness with exponential backoff before allowing the primary app to start.
   * **Primary Running:** Launches the primary application with `DMZ_SIDECAR_SOCK` injected.
3. Click **Stop Pair (Graceful Drain)** to execute KEP-753 reverse teardown:
   * Sends `SIGTERM` to the primary app first.
   * Waits for active IPC traffic to drain.
   * Sends `SIGTERM` to the sidecar and unlinks the ephemeral socket.

### Step 5: Unpack & Run on Clean Air-Gapped Target (GUI Import)
When arriving on a target machine with zero network access:
1. Open `dmz-gui` and navigate to the **Air-Gap Manager** page.
2. In the **Import Air-Gap Archive** card:
   * Click **Open Archive...** and select your `.tar.zst` bundle.
   * Set the **Workspace Directory** in the top card to where you want the project extracted.
   * Ensure **Cryptographically verify closure hash** is checked.
3. Click **Import & Verify Archive**:
   * DMZ verifies archive transport integrity, restores offline dependencies into `.dmz/store`, extracts bundled language toolchains, and restores project source trees with [`dmz-workspace.toml`](file:///Users/rd/Documents/Programming/DMZ/dmz-workspace.toml).
4. Navigate to **Supervisor IPC** and click **Launch Supervised Pair** to run both applications under complete air-gap isolation (`deny network*`).

---

## Security & Best Practices

| Recommendation | Rationale |
| :--- | :--- |
| **Prefer Static Sidecar Binaries (`musl`)** | When building sidecars in Rust, Go, or C++, compile against `musl` (`x86_64-unknown-linux-musl`) to eliminate host dynamic library (`glibc`) mismatches on air-gapped target machines. |
| **Never Use TCP Loopback for Sidecars** | Loopback networking (`127.0.0.1`) requires opening sandbox network rules and risks port collisions. Always use Unix domain sockets or Windows Named Pipes. |
| **Rely on `DMZ_SIDECAR_SOCK`** | Read `DMZ_SIDECAR_SOCK` to ensure compliance with POSIX safe path length guarantees and automated cleanup. |
| **Respect the Drain Window** | When receiving `SIGTERM`, primary apps should finalize active requests before closing the socket, allowing clean shutdown without interrupted state. |
