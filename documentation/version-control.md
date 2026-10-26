# Version Control & Air-Gap Provenance Guide

DMZ embeds native `libgit2` to provide autonomous repository version control, cryptographic provenance tracking, deterministic Git bundle export/import, and incremental delta packaging without requiring a host Git installation.

---

## Core Capabilities

### 1. Cryptographic Provenance Tracking
When resolving or packaging a workspace, DMZ inspects the active repository and captures:
- **Commit Hash:** 40-character SHA-1/OID of `HEAD`.
- **Branch:** Active branch name (or detached `HEAD`).
- **Dirty State:** Whether uncommitted modifications exist in the working tree.

This metadata is recorded directly in `dmz.lock` under the `vcs` field and embedded in `airgap_manifest.json`, providing verifiable traceability across security domains.

### 2. Embedded Git History (`--with-git-history`)
Air-gap packages can optionally include complete or partial Git history by packing a standard Git bundle v2 file (`workspace.bundle`) inside the `.tar.zst` closure:
- Allows fully isolated destination machines to clone or restore the repository offline.
- Compatible with both DMZ's embedded engine and standard host `git clone workspace.bundle`.

### 3. Incremental Delta Transfers (`--since <ref-or-manifest>`)
Transferring multi-gigabyte dependency closures over physical media or data diodes is time- and bandwidth-prohibitive. DMZ supports delta exports:
- Computes added and modified files between the current workspace and a baseline (Git commit hash, Git ref, previous `airgap_manifest.json`, or previous `.tar.zst` archive).
- Packages only changed files and incremental Git commits into a compact delta archive.

### 4. Fail-Fast Delta Verification
When importing an incremental delta archive, the `AirGapImporter` verifies:
1. Target workspace Git commit matches `base_commit`.
2. Target `dmz.lock` closure signature matches `base_closure_signature`.

If a mismatch is detected, the import aborts immediately before unpacking to prevent corrupting non-matching workspaces.

---

## CLI Usage

### Capture Provenance in Lockfile
```bash
# Automatically attaches Git provenance to dmz.lock during resolution
dmz resolve --workspace .
```

### Export Full Package with Git History
```bash
# Creates complete dependency closure + workspace.bundle
dmz bundle --with-git-history --output dist/workspace_full.tar.zst
```

### Export Incremental Delta Package
```bash
# Export delta against a previous Git commit or ref
dmz bundle --since HEAD~1 --output dist/workspace_delta.tar.zst

# Or export delta against a previous air-gap manifest
dmz bundle --since prev_airgap_manifest.json --output dist/workspace_delta.tar.zst

# Or export delta with incremental Git history
dmz bundle --since HEAD~3 --with-git-history --output dist/workspace_delta.tar.zst
```

### Import and Restore on Disconnected Destination
```bash
# Imports delta or full bundle, automatically restoring embedded Git bundle if present
dmz import --archive dist/workspace_delta.tar.zst --to /path/to/target/workspace
```

---

## GUI Control Center

The **Version Control** page provides interactive tooling:

1. **Repository Provenance & Integrity Banner:**
   - Inspect, track, and package repositories without relying on a host Git installation.
      * Displays active commit hash, branch name, and dirty working tree status (clean green vs. amber indicator).
      * "Attach Provenance to dmz.lock" button stamps active VCS metadata into the local lockfile.

2. **Deterministic Git Bundle Export:**
   - Exports repository commits, tree objects, and branch refs into a portable Git bundle v2 file for physical media air-gap transfer.
      * Specify output path (e.g. `workspace.bundle`) and optional baseline commit to generate standalone Git bundle v2 files.

3. **Incremental Delta Packaging & Diff Preview:**
   - Transfers only the files and commits modified since a baseline commit, ref, or previous air-gap bundle, drastically reducing transfer media size.
      * Input a baseline reference (`HEAD~1`, commit hash, or browse for a manifest).
      * Click **Preview Changed Files** to inspect modified/added files before export.
      * Click **Generate Delta Package** to produce an incremental `.tar.zst` archive.

4. **Offline Git Clone / Restore:**
   - Reconstructs an active offline Git repository from a transferred .bundle file using embedded libgit2.
      * Restore a `.bundle` file directly into an offline workspace folder or clone a new repository using embedded `libgit2`.
