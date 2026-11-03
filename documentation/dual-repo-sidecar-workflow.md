# Dual-Repo & Sidecar Workflow in DMZ

> **Guide for orchestrating, sandboxing, and air-gap packaging companion services (Python, Rust, Node, Go) alongside primary applications.**

---

## 1. Overview & Architecture

Modern desktop and full-stack applications often pair a primary frontend/app (e.g. desktop GUI, CLI, or web client) with an independent companion **sidecar** service (e.g. local Python AI worker, Rust/C++ computation backend, or IPC engine).

In unconstrained host environments, these multi-repo projects rely on system-installed dependencies and host networking. In **DMZ**, each repository is hermetically sandboxed, verified with zero host dependencies, and bridged over **daemonless local IPC**:

```
┌────────────────────────────────────────────────────────┐
│                      DMZ Project                       │
│                                                        │
│  ┌──────────────────────┐    ┌──────────────────────┐  │
│  │ Primary Workspace    │    │ Sidecar Workspace    │  │
│  │ (e.g. Node / GUI)    │    │ (e.g. Python / Rust) │  │
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

## 2. Workspace Declaration (`dmz-workspace.toml`)

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

## 3. CLI Developer Workflow

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
Package both source trees, their offline package stores, and compiler sysroots (Node + Python runtime) into a single verifiable `.tar.zst` bundle:

```bash
dmz bundle --with-toolchain --output dist/project_closure.tar.zst
```

### Step 5: Unpack & Run on Clean Air-Gapped Machine
On target air-gapped systems with zero pre-installed runtimes:

```bash
dmz import --archive dist/project_closure.tar.zst --workspace ./project
cd project
dmz shell
```

---

## 4. GUI Developer Workflow

For users preferring visual management in `dmz-gui`:

```
┌────────────────────────────────────────────────────────┐
│  Workspace Mode: [ Single Workspace ] (•) Dual Repo    │
│                                                        │
│  Primary Application Root:                             │
│  [ /Users/.../desktop-ui             ] [ Browse... ]   │
│  Detected: Node.js (package.json)                      │
│                                                        │
│  Sidecar Service Root:                                 │
│  [ /Users/.../ai-engine              ] [ Browse... ]   │
│  Detected: Python (pyproject.toml)                     │
│                                                        │
│  [ Scan & Resolve Both ]  [ Export dmz-workspace.toml] │
└────────────────────────────────────────────────────────┘
```

1. **Select Workspace Mode:**
   * Open the **Resolver** tab.
   * Toggle to **Dual Repo / Sidecar**.
   * Pick the **Primary Application** directory and the **Sidecar Service** directory.
2. **Scan & Resolve:**
   * Click **Scan & Resolve Both**. DMZ analyzes manifests in both folders, displays detected ecosystems, and builds the composite lockfile.
3. **Supervisor IPC Telemetry:**
   * Navigate to the **Supervisor IPC** tab.
   * View live lifecycle states: `Sidecar Initializing` $\rightarrow$ `Socket Handshake Verified` $\rightarrow$ `Primary Running`.
   * Monitor safe socket path telemetry and memory footprint.
4. **Air-Gap Export:**
   * Navigate to the **Air-Gap** tab.
   * Click **Bundle Workspace Closure** to generate the unified `.tar.zst` containing both codebases and toolchains.

---

## 5. Security & Best Practices

| Recommendation | Rationale |
| :--- | :--- |
| **Prefer Static Sidecar Binaries (`musl`)** | When building sidecars in Rust, Go, or C++, compile against `musl` (`x86_64-unknown-linux-musl`) to eliminate host dynamic library (`glibc`) mismatches on air-gapped target machines. |
| **Never Use TCP Loopback for Sidecars** | Loopback networking (`127.0.0.1`) requires opening sandbox network rules and risks port collisions. Always use Unix domain sockets or Windows Named Pipes. |
| **Rely on `DMZ_SIDECAR_SOCK`** | Never hardcode socket paths in source code. Reading `DMZ_SIDECAR_SOCK` ensures compliance with POSIX safe path length guarantees and automated cleanup. |
| **Respect the Drain Window** | When receiving `SIGTERM`, primary apps should finalize active requests before closing the socket, allowing clean shutdown without interrupted state. |

