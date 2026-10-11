# DMZ Command-Line Interface (CLI) Guide

DMZ provides a robust, self-contained CLI for orchestrating zero-dependency sandboxes, managing supervisor services, and executing air-gap closure transfers.

---

## Air-Gap Commands

The air-gap command group (`dmz export` and `dmz import`) allows you to bundle an entire project workspace along with its locked dependency closures into a compressed, cryptographically verifiable archive for transfer across disconnected networks.

### 1. Exporting a Closure (`dmz export`)

Bundles the current workspace, including source files and the `dmz.lock` dependency manifest, into an optimized `.tar.zst` archive.

```bash
dmz export [OPTIONS]
```

#### **Options**

* `-w, --workspace <PATH>` — Path to the source workspace root. *(Defaults to the current working directory)*
* `-o, --output <PATH>` — Destination path for the generated `.tar.zst` archive. *(Defaults to `./closure.tar.zst`)*
* `-c, --compression-level <LEVEL>` — Zstd compression level from `1` (fastest) to `21` (maximum compression). *(Default: `3`)*

#### **Example**

```bash
dmz export --workspace /path/to/project --output /secure/transfer/project-closure.tar.zst --compression-level 9
```

## Air-Gap Export Command (`dmz airgap export`)

Bundles project workspace code, lockfiles, and secure dependency stores into a compressed, cryptographically verifiable `.tar.zst` archive.

### Usage
```bash
dmz airgap export [OPTIONS]
```

---

### 2. Importing & Verifying a Closure (`dmz import`)

Verifies the cryptographic SHA-256 hash of an imported archive, validates the internal `dmz.lock` signature, and safely extracts the workspace onto an offline target machine.

```bash
dmz import [OPTIONS]
```

#### **Options**

* `-a, --archive <PATH>` — Path to the imported `.tar.zst` closure archive. **[Required]**
* `-s, --sha256 <HASH>` — Expected SHA-256 cryptographic checksum of the archive. **[Required]**
* `-t, --target <PATH>` — Destination directory where the workspace and lockfile will be unpacked. **[Required]**

#### **Example**

```bash
dmz import \
  --archive ./project-closure.tar.zst \
  --sha256 4c378397e78464329d42b543b944d2824cdd9856abac26c0f26e323587289ab6 \
  --target /var/lib/dmz/workspaces/offline-project
```

---

## General CLI Usage

To view all available global options, subcommand categories, and version information, run:

```bash
dmz --help
```

---

# DMZ Sandbox Execution & Proxy Management CLI Reference

In addition to air-gap packaging, DMZ provides specialized command groups for secure process isolation (`dmz sandbox`) and network proxy auditing (`dmz proxy`).

---

## Sandbox Execution Commands (`dmz sandbox`)

The sandbox command group manages secure, isolated execution environments with zero host system leakage, ensuring that toolchains, AI agents, or builds run within controlled parameters.

### 1. Running a Command in Isolation (`dmz sandbox run`)

Executes a command inside a secure sandbox instance.

```bash
dmz sandbox run [OPTIONS] -- <COMMAND> [ARGS...]
```

#### **Options**

* `-w, --workspace <PATH>` — Working directory context for the sandbox instance. *(Defaults to current directory)*
* `-e, --env <KEY=VALUE>` — Inject environment variables into the sandbox runtime (can be specified multiple times).
* `--timeout <SECONDS>` — Maximum allowed execution time before forced process termination.
* `--no-net` — Strips all outbound network access for complete runtime air-gapping.
* `--diagnose` — Enables automated error categorization to distinguish between platform/kernel faults and user code/dependency errors, providing plain-language remediation steps.

#### **Example 1**

```bash
dmz sandbox run --workspace ./my-app --diagnose -- cargo build --release
```

#### **Example 2**

```bash
dmz sandbox run --workspace ./my-app --timeout 300 --no-net -- cargo build --release
```

---

### 2. Listing Active Sandboxes (`dmz sandbox list`)

Displays active or backgrounded sandbox processes managed by the local runtime.

```bash
dmz sandbox list
```

---

## Network Proxy Management (`dmz proxy`)

The proxy command group controls the volatile outbound network proxy auditor. It monitors and enforces strict domain whitelists for agentic and tool-driven workloads.

### 1. Starting the Proxy Audit Server (`dmz proxy start`)

Launches the local proxy auditor service to intercept and inspect outbound traffic against your defined security policy.

```bash
dmz proxy start [OPTIONS]
```

#### **Options**

* `-p, --port <PORT>` — Local port for the proxy server. *(Default: `8080`)*
* `-l, --log-file <PATH>` — Path to write request audit trails and violation logs.
* `--strict` — Blocks any unlisted outbound domains by default.

#### **Example**

```bash
dmz proxy start --port 9090 --log-file /var/log/dmz/proxy-audit.log --strict
```

---

### 2. Managing Whitelist Rules (`dmz proxy whitelist`)

Adds or removes approved outbound domains for agent execution.

```bash
dmz proxy whitelist [COMMAND]
```

#### **Subcommands**

* `add <DOMAIN>` — Add a trusted domain to the outbound whitelist (e.g., `github.com`, `crates.io`).
* `remove <DOMAIN>` — Revoke a domain from the whitelist.
* `list` — Display all currently whitelisted domains.

#### **Example**

```bash
dmz proxy whitelist add crates.io
dmz proxy whitelist list
```

---