# Running Tools After Air-Gap Import

After importing an air-gap bundle with `dmz import`, your workspace, locked dependencies, and any bundled toolchains (Rust, Node.js, Python, Go, PHP, Ruby) are automatically unpacked and configured into `.dmz/toolchains/`.

You can run your project's commands, compilers, or test suites in any of the following three ways:

---

## Option 1: Generated Environment Activator

Source the generated environment script to prepend bundled toolchain paths to `PATH` and set standard runtime variables (`CARGO_HOME`, `GOROOT`, `PYTHONHOME`) in your current terminal session:

### macOS / Linux:
```bash
source .dmz/env.sh
<command>
```

### Windows:
```cmd
.dmz\env.bat
<command>
```

**Ecosystem examples:**
```bash
# Rust
cargo build --offline

# Node.js / pnpm / npm
pnpm test
# or: npm start

# Python
python main.py

# Go
go run .
```

---

## Option 2: DMZ Interactive Shell

Open an interactive DMZ subshell. Bundled toolchains, offline dependency store mappings, and environment variables are automatically loaded:

```bash
dmz shell
```

Once inside the interactive shell, run commands directly:
```bash
<command>
```

You can also run a single command directly and exit:
```bash
dmz shell -c "<command>"
```

---

## Option 3: DMZ Isolated Sandbox

Execute commands inside a daemonless, kernel-isolated sandbox (macOS Seatbelt, Linux User Namespaces, Windows Job Objects) with zero network leakage:

```bash
dmz sandbox -- <command>
```

**Ecosystem examples:**
```bash
# Rust
dmz sandbox -- cargo test --offline

# Node.js
dmz sandbox -- pnpm test

# Python
dmz sandbox -- python -m unittest

# Go
dmz sandbox -- go test ./...
```
