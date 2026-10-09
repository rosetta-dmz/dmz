# **Architectural Blueprint**

### **1\. The Statically Compiled Engine (The Core)**

To achieve the "zero dependency factory" where open-source contributors can download a single binary and immediately compile the platform itself, **Rust** is the optimal choice over Go or Zig. Rust provides zero-cost abstractions, deterministic cross-platform compilation, and deeply granular control over system-level process sandboxing (via `libc` and platform-specific kernel APIs).

**The Compilation Strategy:**

* **Target:** `x86_64-unknown-linux-musl` (Linux), `x86_64-pc-windows-msvc` (Windows), and `aarch64-apple-darwin` (macOS).  
* **Static Linking (`musl`):** By compiling the Linux binary against `musl` instead of the standard `glibc`, you completely sever the binary's dependence on the host machine's C library version.  
* **Embedded Dependencies:** You link core functionalities directly into the binary rather than relying on host OS commands.  
  * *Git:* Embed `libgit2` (via the `git2-rs` crate) to handle repository cloning, hashing, and version control without requiring `git` to be installed on the host.  
  * *Archive/Compression:* Embed `tar` and `zstd` compression crates natively so the engine can pack and unpack `.nar` closures and OCI images offline.

### **2\. The Daemonless Sandbox Engine (The Workspace)**

Because we are stripping out the Docker daemon, the engine must leverage the native isolation features of the host operating system directly. When a developer clicks "Start Workspace", the single Rust binary acts as the local orchestrator:

* **Linux (Native Namespaces):** The Rust engine uses the `unshare` system call to create a new, unprivileged Linux User Namespace. It then uses `chroot` or `pivot_root` to trap the process inside the read-only `/nix/store` dependency closure, completely isolating the development environment from the rest of the host file system.  
* **macOS (Native Sandbox):** The engine leverages the macOS `sandbox-exec` utility and Seatbelt profiles. It writes a strict, temporary `.sb` (sandbox profile) file that grants the development process read/write access *only* to the specific project repository folder and read-only access to the local dependency closure.  
* **Windows (WSL2 or Job Objects):** For deep isolation, the Rust engine automatically orchestrates the environment inside a lightweight WSL2 instance (which is effectively native on modern Windows). Alternatively, for pure native execution, it uses Windows Job Objects and Restricted Access Tokens to lock down the process tree.

### 

### **3\. The AI Agent Sandbox (The Supervisor)**

AI agents require continuous execution, network access, and isolated scratchpads. To accommodate this without a global system daemon, the architecture introduces a **Workspace Supervisor**—an isolated, lightweight thread spawned by the main Rust binary when the environment starts.

**Agent Isolation Constraints:**

* **Process Confinement:** The agent is launched as a child process inside the same restricted User Namespace or Seatbelt sandbox as the development environment. It cannot escape to view the developer's personal host files.  
* **Network Proxies:** If the AI agent needs to access the internet (e.g., to reach OpenAI APIs), the Supervisor intercepts those calls. The Rust engine can enforce network whitelists, ensuring the agent cannot execute unauthorized data exfiltration or scrape internal corporate subnets.  
* **Ephemeral Scratchpad:** The agent is given a dedicated, volatile `tmpfs` (in-memory file system) for its working memory. When the developer closes the workspace, the Supervisor kills the agent's process tree and the `tmpfs` vanishes instantly, leaving zero residual state or corrupted files behind.