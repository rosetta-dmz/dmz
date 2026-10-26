# CLI Walkthrough

## CLI Ergonomics & Subcommands

```bash
# Initialize a workspace
dmz init --target macos-arm64

# One-shot bundle export (resolve + fetch + export)
dmz bundle --target linux-x86_64 --output dist/app.tar.zst

# Import and restore an air-gap bundle
dmz import --archive dist/app.tar.zst --to /opt/deploy

# Run inside daemonless sandbox
dmz shell
```

### CLI Commands

| Command | Usage | Description |
| :--- | :--- | :--- |
| `dmz bundle` | `dmz bundle [-t <target>] [-o <out>]` | One-shot resolution, fetching, and air-gap export (`.tar.zst`) |
| `dmz import` | `dmz import -a <archive> [-t <to>]` | Unpacks, cryptographically verifies, and installs dependencies |
| `dmz shell` | `dmz shell [-c <cmd>] [--no-sandbox] [--allow-network]` | Interactive terminal inside daemonless sandbox (sandboxed & offline by default) |
| `dmz init` | `dmz init [-w <workspace>] [-t <target>]` | Scans workspace and initializes baseline `dmz.lock` |
| `dmz resolve` | `dmz resolve [-t <target>]` | Manifest analysis and dependency graph closure generation |
| `dmz fetch` | `dmz fetch` | Fetches, authenticates, and caches locked artifacts into local store |
| `dmz install` | `dmz install` | Offline dependency injection into project-native layouts |
| `dmz sandbox` | `dmz sandbox exec -- <cmd>` | Low-level direct sandbox process invocation |
| `dmz airgap` | `dmz airgap export / import` | Low-level archive packaging and extraction |
