# Air-Gapped Packaging Pipeline

With the air-gap packaging pipeline, cryptographic verification, and `.tar.zst` closure extraction tested and finalized, DMZ can package a complete codebase alongside all its dependency closures into a single, highly compressed, cryptographically signed archive for secure transfer to fully disconnected target machines.

---

### How It Works

1. **Closure Resolution & Locking:** DMZ resolves all dependencies across ecosystems (such as Cargo, npm, pnpm, Node, etc.) and records their exact source URLs, versions, and checksums into `dmz.lock`.
2. **Deterministic Archival:** Using the core packaging engine and `zstd` compression, the workspace and its target dependencies are bundled into an isolated `.tar.zst` closure archive.
3. **Cryptographic Validation:** When transferring to an air-gapped target machine, the archive is verified against its expected SHA-256 hash and lockfile signature before unpacking:
```bash
dmz import --archive closure.tar.zst --sha256 <expected_hash> --target /path/to/target/workspace
```


4. **Zero External Setup:** Because DMZ embeds `libgit2` and handles extraction natively, the target machine requires **no pre-installed toolchains, package managers, or internet access** to reconstruct the exact workspace state.

---