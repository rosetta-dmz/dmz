# DMZ Hermetic Toolchain: Capabilities & Clear Constraints

## Capabilities (What Is Fully Supported)

* **14 Autonomously Discovered Toolchain Ecosystems:**
  - **Languages & Runtimes:** Rust, Go, Python, Node.js, Ruby, PHP.
  - **Native Build Generators:** Kitware CMake, Ninja, GNU Make, Juce Projucer, PkgConfig.
  - **C/C++ Compilers & Linkers:** Standalone Clang/LLVM, GCC, `llvm-ar`, `ar`, `llvm-ranlib`, `ranlib`, `lld`, `ld`, and compiler builtin header packages (`clang -print-resource-dir`).
  - **Mobile & Platform SDKs:** Android NDK (Clang cross-compilers, sysroots, and `android.toolchain.cmake`), Apple Developer Toolchains (Xcode Command Line Tools, Clang, Swift, `xcodebuild`, `xcrun`, and macOS/iOS SDKs).

* **Zero-Configuration Autonomous Discovery:**
  - Filesystem manifest scanning up to directory depth 3 without requiring user flags.
  - Automatic dependency inspection across lockfiles (`dmz.lock`, `Cargo.lock`) and vendored packages.
  - Companion toolchain inference engine automatically resolves dependency cascades:
    - $\text{CMake} \implies \text{Cc} + \text{Ninja}$ (or $\text{Make}$)
    - $\text{Make} / \text{Ninja} \implies \text{Cc}$
    - $\text{Juce} \implies \text{Cc} + \text{CMake}$
    - $\text{Android NDK} \implies \text{CMake} + \text{Ninja}$
    - $\text{Apple SDK} \implies \text{Cc}$

* **Standard-Compliant Hermetic Generation:**
  - Autonomously emits `.dmz/toolchains/dmz.toolchain.cmake` configuring compilers, archivers, search roots, Apple `CMAKE_OSX_SYSROOT`, and chaining Android NDK toolchains.
  - Retains full standard executable dynamic linking and ABI inspection; does not employ bare-metal bypasses (`CMAKE_TRY_COMPILE_TARGET_TYPE="STATIC_LIBRARY"`).

* **Air-Gap Materialization & Permission Handling:**
  - Unpacks into local workspace directory `.dmz/toolchains/`.
  - Automatically enforces `0o755` executable permissions across all `bin/` directories.
  - Recursively strips macOS Gatekeeper quarantine xattrs (`com.apple.quarantine`).
  - Generates standalone activator scripts (`.dmz/env.sh` and `.dmz/env.bat`).
  - Injects variables into all DMZ execution contexts (`dmz shell`, `dmz verify`, and sandbox runners).

---

## Boundaries & Constraints (What Is Not Supported)

### Constraint 1: Host OS & CPU Architecture Homogeneity
* **Constraint:** Bundled host toolchain binaries (`clang`, `cmake`, `ninja`, `rustc`, `node`, etc.) execute natively on the target host CPU architecture and operating system kernel.
* **Boundary:** An air-gap archive packaged on macOS Apple Silicon (`darwin-aarch64`) cannot run its host toolchain binaries on an x86_64 Linux target machine. Target machines must share a compatible OS kernel and CPU architecture with the export machine (or cross-compilation target).

### Constraint 2: Kernel & Fundamental C Library (`libc`) Baseline
* **Constraint:** DMZ operates entirely in user-space and does not replace the target machine's operating system kernel or base dynamic linker / C runtime (`glibc.so.6` / `musl` / macOS `libSystem.B.dylib`).
* **Boundary:** On Linux, compiled binaries packaged on a system with a newer `glibc` version (e.g. `glibc 2.38`) cannot execute on an older target host with an outdated runtime (e.g. `glibc 2.31`). The target host must meet or exceed the minimum glibc baseline of the packaged binaries.

### Constraint 3: Root-Managed System Path Relocatability
* **Constraint:** Host compilers installed into non-relocatable root directories (e.g. `/usr/bin/gcc`, `/usr/lib64`) have hardcoded library search paths and lack standalone isolation.
* **Boundary:** DMZ detects and packages standalone toolchains cleanly. When only root-managed non-relocatable system compilers are found, DMZ emits an advisory notice recommending standalone/hermetic distributions (e.g. LLVM toolchains, rustup toolchains, standalone NDK).

### Constraint 4: User-Space Execution (No Root/Kernel Modules)
* **Constraint:** DMZ runs under standard unprivileged user credentials within the `.dmz/` workspace.
* **Boundary:** Builds requiring root privileges (`sudo`), kernel module insertion, hardware loop mounts, or direct device driver manipulation cannot be executed via user-space sandboxing.

### Constraint 5: Proprietary SDK Licensing & Hardware Drivers
* **Constraint:** Apple Developer SDKs, Xcode Command Line Tools, and proprietary graphics/compute runtimes (e.g. NVIDIA CUDA drivers) are subject to vendor license agreements and hardware driver presence.
* **Boundary:** DMZ discovers and packages existing SDKs on the export machine. It does not license Apple or NVIDIA proprietary software, nor does it emulate GPU hardware drivers on air-gapped target machines lacking physical hardware support.
