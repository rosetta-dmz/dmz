# Roadmap

This document outlines the architectural roadmap, current implementation status, and upcoming milestones for the DMZ Platform.

---

## Current Platform Status (v1.4.1+)

DMZ is a zero-dependency, daemonless workspace packaging engine and environment manager designed for deterministic, air-gapped software development.

### Implemented Functionality

* **14-Ecosystem Discovery & Verification:**
  Full discovery, lockfile parsing, and offline validation across 14 ecosystems:
  * **Primary Runtimes & Languages:** Rust (`cargo`), Node.js (`npm`, `pnpm`, `yarn`), Python (`pip`, `venv`), Go, PHP (`composer`), Ruby (`bundler`).
  * **Native & Auxiliary Build Systems:** C/C++ (`clang`, `gcc`, `llvm-ar`, `ranlib`), CMake, Make, Ninja, `pkg-config`, JUCE audio framework.
  * **Platform SDKs:** Apple SDK (Darwin sysroot, framework harvesting), Android NDK (CRT objects, cross-compilers).
* **Hermetic Toolchain Packaging & Auto-Setup (`--with-toolchain`):**
  * Bundled toolchains are packaged by default on export (`dmz bundle` / `dmz airgap export`), with `--no-toolchain` opt-out.
  * Captures runtime binaries, standard libraries, header trees, and companion dynamic libraries (`libtapi.dylib`, `libLTO.dylib` for macOS Mach-O `@rpath` relocation).
  * Automatically generates Clang hermetic wrappers, unified CMake toolchain configurations (`dmz.toolchain.cmake`), environment activators (`.dmz/env.sh`, `.dmz/env.bat`), and IDE settings (`.vscode/settings.json`, `.cargo/config.toml`).
  * Enforces **Root Manifest Dominance** to eliminate foreign toolchain hallucination.
* **Air-Gap Bundle Packaging (`.tar.zst`):**
  * Target A (Dependencies), Target B (App Source), and Target C (Unified Closure).
  * Streaming zstd compression, cryptographic SHA-256 sidecar generation, and tamper detection.
  * Fail-fast store verification, path traversal prevention, and transactional rollback on import failure.
* **Dual-Repo & Multi-Workspace Umbrella Architecture:**
  * Dynamic workspace orchestration supporting multi-repo topologies and companion services (`dmz-workspace.toml`).
  * Umbrella archive packaging and unified import with virtual runfiles tree mapping (`workspaces/<id>`).
  * External sidecar bundling with KEP-753 lifecycle sequencing, health probing, and zero-network local IPC (`AF_UNIX` sockets / Windows Named Pipes).
  * Cross-platform member parity links with directory junctions, hardlinks, and generic CLI shim hydration (`.dmz/bin`).
* **High-Performance Fast-Clone Engine (Reflink / CoW):**
  * Zero-cost file cloning via OS-level Copy-on-Write reflinks: APFS `clonefile` on macOS and Linux `ioctl(FICLONE)` / `BTRFS_IOC_CLONE`.
  * Transparent fallbacks to hardlinks and buffered copy with Windows read-only attribute remediation.
  * Drastically accelerates workspace snapshotting, offline materialization, and dependency linking.
* **Modular CLI Architecture (SOLID Refactoring):**
  * Decomposed monolithic CLI dispatch into 12 dedicated, single-responsibility subcommand handlers in `dmz-cli/src/commands/`:
    `resolve`, `fetch`, `install`, `build`, `sandbox`, `export` (`airgap`), `bundle`, `import`, `shell`, `init`, `ecosystem`, `run`.
  * Decoupled argument parsing from domain command execution for enhanced testability and clean domain boundaries.
* **Organized & Domain-Partitioned Test Suite:**
  * Fully restructured test infrastructure organized under `dmz-core/tests/` into dedicated functional domains:
    `airgap-pipeline/`, `airgap-validation/`, `dual-repo-umbrella/`, `hardening/`, `offline-install/`, `vcs-delta/`, and `workspace-resolution/`.
  * Comprehensive regression verification covering offline roundtrips, multi-workspace umbrella packaging, and foreign build system hardening (Nix, Bazel).
* **VCS Provenance & Delta Synchronization:**
  * Embedded Git provenance, commit tracking, and dirty-state detection in `dmz.lock` and `airgap_manifest.json`.
  * Incremental delta packaging (`--since <commit>`) with base-commit mismatch guards.
  * Deterministic Git bundle v2 packaging (`--with-git-history`).
* **Daemonless Process Sandboxing:**
  * **macOS:** Apple Seatbelt profile confinement (`sandbox-exec`) with fine-grained path allowlists and Mach-O `@rpath` dynamic linker preservation.
  * **Windows:** Win32 Job Objects (`CreateJobObjectW`, `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) ensuring clean child process termination and process tree containment.
  * **Linux:** Unprivileged namespaces, `chroot`/`pivot_root`, and cgroup limits with robust fallback handling.
  * **Universal Cache Isolation:** Redirects build caches across all ecosystems into `.dmz/cache`.
* **AI Agent Supervisor & Egress Proxy:**
  * Background workspace supervisor thread with cross-platform IPC (Unix domain sockets / Windows Named Pipes).
  * Ephemeral scratchpads (`EphemeralFs`) with automatic teardown on drop.
  * Dedicated network proxy (`DomainWhitelist`) with wildcard domain routing and HTTP proxy auditing.
* **Native GUI Application (`dmz-gui`):**
  * Visual inspection of airgap closures, active sandboxes, supervisor lifecycle, and version control.
  * Multi-theme support (`mac_light`, `field_ops`) and persistent settings.

---

## Active & Upcoming Milestones

### Milestone v1.5 (Short-Term Focus)

* **Deep ABI & Dynamic Linker Analysis:**
  * Static inspection of native binaries, Mach-O load commands, and ELF `.dynamic` sections.
  * Pre-flight validation of glibc/musl minimum symbol versions and C++ standard library runtime compatibility in offline wheels and npm native addons before execution.
* **System Package Stack Analysis:**
  * Discovery, capture, and verification of OS-level packages (`apt`, `brew`, `dnf`) alongside language package managers.
  * Generating host package dependency declarations for air-gapped target deployment.
* **Linux Namespace Confinement Hardening:**
  * Direct unprivileged Linux User Namespaces (`unshare` with `CLONE_NEWUSER`, `CLONE_NEWNS`, `CLONE_NEWNET`, `CLONE_NEWPID`).
  * Robust fallback handling for locked-down kernel environments (e.g., restricted `kernel.unprivileged_userns_clone`).

### Milestone v2.0 (Long-Term Vision)

* **OCI / Docker Container Image Export:**
  * Converting locked dependency closures and workspace snapshots into standard OCI layer tarballs, image manifests, and config JSON (`application/vnd.oci.image.manifest.v1+json`).
  * Creating runnable container images exportable to `docker load` or standard container registries without requiring an active Docker daemon.
* **Automated Target VM Pre-flight Bundle Testing:**
  * Automated spin-up of target-compatible lightweight micro-VMs (e.g. Firecracker or QEMU) to execute pre-flight bundle compilation and verify runtime ABI compatibility before physical deployment.
* **Cross-Architecture Native Emulation:**
  * Embedded user-mode emulation to validate cross-compiled bundles on non-matching host architectures.

---

## Status Matrix

| Capability | Current Status (v1.4.1+) | Primary Files |
| :--- | :--- | :--- |
| **Multi-Ecosystem Discovery** | **Implemented** (14 ecosystems: Rust, Node, Python, Go, PHP, Ruby, CC, CMake, Make, Ninja, Apple SDK, Android NDK, JUCE, pkg-config) | `dmz-core/src/ecosystem/`, `dmz-core/src/toolchain/discovery/` |
| **Native Toolchain Packaging** | **Implemented** (Bundled by default; Clang wrappers, CMake toolchain, env generation) | `dmz-core/src/toolchain/packager.rs`, `dmz-core/src/toolchain/setup/` |
| **Native Ecosystem Verification** | **Implemented** (`dmz ecosystem verify` across all ecosystems) | `dmz-core/src/ecosystem/handlers.rs`, `dmz-cli/src/commands/ecosystem.rs` |
| **Target Platform Compatibility Check** | **Implemented** (Lockfile/manifest tags + import warning & rollback) | `dmz-core/src/archive/importer/`, `dmz-core/src/resolver/lockfile.rs` |
| **Air-Gap Bundle Packaging (`.tar.zst`)** | **Implemented** (Target A, B, C; CLI & GUI with SHA-256 sidecar) | `dmz-core/src/archive/builder.rs`, `dmz-core/src/archive/exporter/` |
| **Dual-Repo & Umbrella Architecture** | **Implemented** (Multi-workspace topology, companion sidecars, KEP-753 probes) | `dmz-core/src/workspace/`, `dmz-supervisor/src/sidecar-orchestrator/` |
| **Fast-Clone (Reflink / CoW) Engine** | **Implemented** (APFS `clonefile`, Linux `FICLONE`, Win32 fallback) | `dmz-core/src/install/fast_clone.rs` |
| **Modular CLI SOLID Architecture** | **Implemented** (12 decoupled subcommand modules in `dmz-cli/src/commands/`) | `dmz-cli/src/commands/`, `dmz-cli/src/cli.rs` |
| **Domain-Partitioned Test Suite** | **Implemented** (Partitioned test domains in `dmz-core/tests/`) | `dmz-core/tests/` |
| **Embedded Git Provenance & Status** | **Implemented** (`dmz.lock` & `airgap_manifest.json` tracking) | `dmz-core/src/vcs/provider.rs`, `dmz-core/src/resolver/lockfile.rs` |
| **Incremental Delta Air-Gap Bundles** | **Implemented** (`--since` delta diffs & import baseline check) | `dmz-core/src/archive/exporter/`, `dmz-core/src/archive/importer/` |
| **Deterministic Git Bundle Inclusion** | **Implemented** (`--with-git-history` Git bundle v2 packaging) | `dmz-core/src/vcs/provider.rs`, `dmz-core/src/archive/importer/` |
| **Daemonless Process Sandbox** | **Implemented** (macOS Seatbelt, Windows Job Objects, Linux namespaces) | `dmz-sandbox/src/platform/` |
| **AI Agent Supervisor & Egress Proxy** | **Implemented** (Domain whitelist, wildcard proxy, ephemeral scratchpad) | `dmz-sandbox/src/agent/`, `dmz-supervisor/` |
| **GUI Version Control & Airgap UI** | **Implemented** (Dedicated airgap, sandbox, VCS, and settings tabs) | `dmz-gui/src/app/pages/` |
| **Deep ABI / Binary Header Analysis** | **Planned** (Static glibc/musl & dynamic linker symbol verification) | Roadmap v1.5 | `documentation/specifications/` |
| **System Package Capture (apt, brew, dnf)** | **Planned** (OS package stack capture & declaration) | Roadmap v1.5 | `documentation/about/what-is-dmz.md` |
| **Linux Namespace Hardening** | **In Progress** (Direct unprivileged user namespaces & locked kernel fallbacks) | Roadmap v1.5 | `dmz-sandbox/src/platform/linux.rs` |
| **OCI / Container Image Export** | **Planned** (Daemonless OCI manifest & layer tarball export) | Roadmap v2.0 | `documentation/specifications/` |
| **Automated Target VM Bundle Pre-flight** | **Planned** (Lightweight micro-VM pre-flight compilation) | Roadmap v2.0 | `documentation/about/what-is-dmz.md` |
| **Cross-Architecture Native Emulation** | **Planned** (User-mode emulation for cross-compiled bundle validation) | Roadmap v2.0 | `documentation/specifications/` |
