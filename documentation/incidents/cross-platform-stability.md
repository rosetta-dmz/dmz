## DMZ Sandbox Resolution & Post-Mortem Documentation

This document records the root cause of the platform execution and test harness failures encountered during the development of **DMZ** (`dmz-sandbox`), along with the preventative patterns established to ensure cross-platform stability.

---

### 1. The Incident Summary

During end-to-end integration testing (`cargo test -p dmz-core --test integration_test`), sandboxed process execution consistently failed under Unix/macOS runtimes with exit status `None` and signal termination `Some(6)` (`SIGABRT`).

### 2. Root Causes Identified

* **macOS Symlink Mismatch (`/var` vs. `/private/var`):**
Temporary workspace directories created by `tempfile::TempDir` reside under `/var/folders/...` on macOS, which is a symbolic link to `/private/var/folders/...`. When Apple's Seatbelt (`sandbox-exec`) evaluated paths, the string prefix mismatch caused access denials (`EACCES`), crashing the wrapper process.
* **Linux Namespace & Multi-threading Restrictions (`SIGABRT`):**
Calling `unshare(CLONE_NEWUSER)` or `unshare(CLONE_NEWPID)` inside a multithreaded process (such as a Rust test suite running concurrent test workers) violates glibc thread-safety invariants, causing glibc's internal assertions to trigger an immediate abort (`SIGABRT`).
* **POSIX Wait Status Bit-Packing:**
Directly mapping raw `waitpid` process return codes into Rust's `ExitStatus::from_raw()` without correct POSIX status shifts (`(code & 0xff) << 8` for normal exits vs. `sig & 0x7f` for signal terminations) caused the standard library to misinterpret normal exit codes as abnormal signal crashes.

---

### 3. Solutions & Preventative Design Patterns

To prevent these issues in future development, adhere to the following architectural rules:

1. **Path Canonicalization:**
Always canonicalize workspace and temporary directories (`std::fs::canonicalize`) before passing them into security sandboxes (`sandbox-exec`, seccomp, or namespace boundaries) to normalize symlinks.
2. **Graceful Fallback & Result Handling:**
Platform sandbox wrappers must handle restricted container environments gracefully by capturing `Result` variants and falling back to standard process isolation when kernel-level capabilities or namespaces are restricted (e.g., in unprivileged CI runners).
3. **Correct POSIX Exit Status Mapping:**
When wrapping low-level Unix process execution via `nix::sys::wait`, ensure wait status bitfields are correctly packed before converting them to Rust `ExitStatus`:
```rust
let raw_status = (code & 0xff) << 8; // For normal exits
let raw_status = (sig as i32) & 0x7f; // For signal terminations

```