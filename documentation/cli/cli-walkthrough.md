# CLI Walkthrough

## CLI Ergonomics & Subcommands

```bash
# Initialize a workspace
dmz init --target macos-arm64

# One-shot bundle export (resolve + fetch + export)
dmz bundle --target linux-x86_64 --output dist/app.tar.zst

# Import and restore an air-gap bundle
dmz import --archive dist/app.tar.zst --to /opt/deploy

# Verify active ecosystems offline
dmz ecosystem verify --workspace /opt/deploy

# Run inside daemonless sandbox
dmz shell
```

### CLI Commands

| Command | Usage | Description |
| :--- | :--- | :--- |
| `dmz bundle` | `dmz bundle [-t <target>] [-o <out>] [--toolchain-profile minimal\|full] [--include-dev]` | One-shot resolution, fetching, and minimal-closure air-gap export (`.tar.zst`) |
| `dmz import` | `dmz import -a <archive> [-t <to>] [--include-dev]` | Unpacks, cryptographically verifies, prunes unrequested toolchains/store files, installs dependencies, and prints tailored next steps |
| `dmz run` | `dmz run [-w <workspace>] [--sidecar <path>]` | Launches dual-repo / companion workspaces under coordinated KEP-753 IPC supervision (run from host shell, not inside `dmz shell`) |
| `dmz ecosystem` | `dmz ecosystem discover / verify` | Discovers active stacks and runs offline verification (`cargo check --offline`, `npm ls --offline`, `python3 -m compileall -q .`, `go build -mod=vendor ./...`) |
| `dmz shell` | `dmz shell [-c <cmd>] [--no-sandbox] [--allow-network] [--strict-metadata]` | Interactive terminal inside daemonless sandbox (sandboxed & offline by default) |
| `dmz init` | `dmz init [-w <workspace>] [-t <target>] [--include-dev]` | Scans workspace and initializes baseline `dmz.lock` |
| `dmz resolve` | `dmz resolve [-t <target>] [--include-dev]` | Manifest analysis and minimal dependency graph closure generation |
| `dmz fetch` | `dmz fetch` | Fetches, authenticates, and caches locked artifacts into local store |
| `dmz install` | `dmz install` | Offline dependency injection into project-native layouts |
| `dmz sandbox` | `dmz sandbox [--strict-metadata] -- <cmd>` | Low-level direct sandbox process invocation |
| `dmz airgap` | `dmz airgap export / import [--toolchain-profile minimal\|full] [--include-dev]` | Low-level archive packaging and extraction |
