# Air-Gapped Packaging Pipeline

DMZ packages complete codebases alongside their dependency closures into single, highly compressed, cryptographically signed `.tar.zst` archives for secure transfer to fully disconnected target machines.

---

### Core Pipeline Capabilities

1. **Closure Resolution & Locking:** DMZ resolves all dependencies across ecosystems (Cargo, npm, pnpm, Python wheels, Go modules, etc.) and records their exact source URLs, versions, authentic SHA-256 digests, and active Git commit provenance into `dmz.lock`.
2. **Deterministic Archival:** Using the core packaging engine and Zstandard compression, the workspace and target dependencies are bundled into an isolated `.tar.zst` closure archive.
3. **Embedded Git History (`--with-git-history`):** Optionally archives a deterministic Git bundle v2 file (`workspace.bundle`) directly inside the package, allowing offline targets to retain full commit history without host Git.
4. **Incremental Delta Transfers (`--since <ref-or-manifest>`):** Transfers only modified files and commits across physical media or data diodes, drastically reducing transfer volume for large repositories.
5. **Zero External Setup:** Because DMZ embeds `libgit2` and handles extraction natively, the target machine requires **no pre-installed toolchains, package managers, or internet access** to reconstruct the exact workspace state.
6. **Native Toolchain Packaging (`--with-toolchain`):** Automatically captures active host compilers and runtimes (Rust `cargo`/`rustc` and sysroot libraries, Node.js, Python, Go, PHP, Ruby) directly into the `.tar.zst` archive. On the air-gapped target, DMZ automatically extracts them into `.dmz/toolchains/`, clears OS quarantines (`xattr -cr` on macOS), and generates ready-to-source `.dmz/env.sh` and `.dmz/env.bat` environment activators.
7. **Cryptographic Validation:** When transferring to an air-gapped target machine, the archive is verified against its expected SHA-256 hash, baseline delta signature, and lockfile signature before unpacking:

```bash
# Full bundle export with Git history and compiler toolchains
dmz bundle --with-git-history --with-toolchain --output dist/workspace_closure.tar.zst

# Incremental delta export against a baseline commit or previous bundle
dmz bundle --since HEAD~1 --output dist/workspace_delta.tar.zst

# Import on air-gapped destination (unpacks workspace, dependencies, and toolchains)
dmz import --archive dist/workspace_closure.tar.zst --workspace /path/to/target/workspace

# Activate the bundled compiler toolchains on the disconnected target
cd /path/to/target/workspace
source .dmz/env.sh       # Or enter isolated environment via: dmz shell
cargo run -p dmz-gui
```