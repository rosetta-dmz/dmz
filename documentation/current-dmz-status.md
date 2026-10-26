# DMZ Platform Status & Audit Record

## 10/20/26: Platform Resolution & Maturity Update

### Resolution Summary Matrix

| Claim / Component | 10/15 Assessment | 10/20 Resolved Status | Evidence & Changes |
|---|---|---|---|
| **Workspace & Closure Packaging** | Partial | **Resolved & Documented** | Clarified DMZ's role as a source + dependency closure packager (`.tar.zst`). Build artifacts (`target`, `node_modules`, `dist`, `.dmz`) are filtered during export; dependencies are materialized deterministically on import via `InstallerEngine`. `exporter.rs`, `importer.rs` |
| **Multi-Ecosystem Detection** | Partial | **Resolved & Aligned** | Handlers support Rust, npm, pnpm, Yarn, Go, and Python. Manifest scanner supports additional ecosystems (Composer, Bundler). System package capture (apt/brew) cleanly categorized as a Roadmap v1.1 milestone. `what-is-dmz.md`, `handlers.rs` |
| **`dmz.lock` Target Platform & Checksums** | Mostly unsupported | **Resolved** | Added `target_platform` to `DmzLockfile` and `AirGapManifest`. In `fetch_dependencies`, authentic SHA-256 artifact hashes are written back into `dmz.lock` and closure signature is recomputed. In `InstallerEngine`, verification recognizes authentic vs identity signatures. `lockfile.rs`, `mod.rs`, `install/mod.rs` |
| **Offline Bundle Testing & Compatibility** | Unsupported | **Target Validation Added** | `AirGapImporter` now validates the bundle's `target_platform` against the receiving host platform (`OS-ARCH`) and warns if incompatible. Pre-flight VM testing scheduled for v1.2. `importer.rs` |
| **CLI Ergonomics (`init`, `bundle`, `import`, `shell`)** | Mostly unsupported | **Resolved** | Added top-level commands `dmz bundle` (with `--target`), `dmz import` (with `--to`), `dmz init`, and `dmz shell` to `dmz-cli`, unifying the user experience with documentation. `main.rs` |
| **GUI Bundle Workflow** | Partial / mostly present | **Verified Stable** | `dmz-gui` AirGap transfer page verified and linked with `ExportConfig.target_platform`. `airgap.rs` |
| **OCI Image Export** | Unsupported | **Moved to Roadmap** | Removed unsupported OCI export claims from core documentation; listed under Platform Roadmap (v1.2). `what-is-dmz.md` |

---

### Test Suite Quality Gate (10/20/26)

- **`cargo check --workspace --all-targets`** — **PASSED** (0 errors, 0 warnings).
- **`cargo test --workspace`** — **100% PASSED** (25 passed; 0 failed across all 5 workspace crates).
  - `dmz-core`: 14 passed (Resolved: missing `src/lib.rs` in `workspace_resolution_test.rs`; fixed `.cargo/config.toml` vendored directory path and `.cargo-checksum.json` creation in `air_gap_install_integration.rs`).
  - `dmz-gui`: 10 passed.
  - `dmz-sandbox`: 1 passed.
  - `dmz-supervisor`: 0 passed (unit/doc).
  - `dmz-cli`: verified CLI commands and help outputs.

---

## 10/15/26: Historical Initial Audit (Reference)

Currently, DMZ has real air-gap archive/export/import workflows, a GUI transfer page, ecosystem discovery, and a lockfile. However, the core goal—capturing and restoring a complete, compatible development environment—does not match the implementation.

| Claim in `what-is-dmz.md` | Assessment | Evidence |
|---|---|---|
| Captures source workspace and development environment into a portable bundle | **Partial** | Export includes workspace files plus store contents, but skips `.git`, `target`, `node_modules`, `dist`, and `.dmz`. It does not capture runtimes, system libraries, or the installed host environment. `exporter.rs` |
| Detects Node/pnpm, Python, Rust, and system packages | **Partial** | CLI ecosystem handlers cover Rust, pnpm, Yarn, npm, Go, and Python; system package detection is absent. Manifest parsing has some additional ecosystems, but the CLI discovery registry does not. `ecosystem/mod.rs` `handlers.rs` |
| `dmz.lock` fingerprints captured components and the bundle target platform | **Mostly unsupported** | Lock entries have package hashes and a closure digest, but no platform/target field. Resolution initializes package hashes from dependency identity; fetch writes computed hashes to per-package metadata rather than back into `dmz.lock`. Offline install warns on hash mismatch and continues. `lockfile.rs` `resolver/mod.rs` `install/mod.rs` |
| Tests the bundle offline in a target-compatible environment | **Unsupported** | Bundle/export/import and checksum/closure checks exist, as do ecosystem verification commands, but there is no target-environment bundle test or platform compatibility validation. Import verifies the lockfile closure hash, not compatibility with the receiving host. `importer.rs` |
| Sandboxed `dmz open`, `dmz shell`, and `dmz run`; host access/network controlled by policy | **Mostly unsupported / platform-dependent** | CLI exposes `dmz sandbox <command>`, not `open`, `shell`, or `run`. macOS uses a Seatbelt profile; Linux and Windows platform implementations launch the process directly, so the claimed isolation and network-policy behavior is not cross-platform. `main.rs` `macos.rs` `linux.rs` `windows.rs` |
| GUI bundle workflow and CLI capture/restore | **Partial / mostly present** | GUI has Export bundle and Import bundle actions. CLI supports `dmz airgap bundle`, `export`, and `import`, with workspace/store options. But these transfer files and dependency artifacts rather than restore a complete captured host environment. `airgap.rs` `main.rs` |
| OCI image export | **Unsupported in inspected implementation** | No OCI/Docker/Podman image export path was found in the CLI, core, or GUI. |

### Checks (10/15/26)
- `cargo check --workspace` — passed.
- Fresh CLI help confirms `resolve`, `fetch`, `install`, `build`, `sandbox`, `airgap`, and `ecosystem`; `init`, `open`, `shell`, and `run` are not exposed. `airgap` does expose `bundle`, `export`, and `import`.
- Air-gap roundtrip/import and package export/import integration tests passed in the runs performed.
- Other test failures: Rust offline-install integration failed because Cargo reported a fixture manifest had no targets; workspace-resolution tests had 2 failures out of 5; the sandbox end-to-end test was blocked by macOS `sandbox_apply: Operation not permitted` in this environment.