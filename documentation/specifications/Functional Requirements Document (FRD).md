# **System Requirements Specification (SRS)**

**Project Concept:** Universal Daemonless Environment & Packaging Engine (UDEDE)

## **I. Functional Requirements (FR)**

### **1\. Zero-Dependency Bootstrap & Embedded Version Control**

* **FR-1.1:** The platform shall be distributed as a single, statically linked binary requiring zero external host dependencies (e.g., no pre-installed Python, Node, Docker, Nix, or Git).  
* **FR-1.2:** The engine shall embed native repository management libraries (e.g., `libgit2`) to handle cloning, branching, and commit state without relying on the host operating system’s Git installation.  
* **FR-1.3:** The platform shall be capable of compiling its own next version directly from its source code using its own internal dependency resolver and sandbox (self-hosting capability).

### **2\. Language-Agnostic Dependency Management**

* **FR-2.1:** The engine shall scan project directories to automatically detect dependency manifests (e.g., `package.json`, `requirements.txt`, `Cargo.toml`, `go.mod`) across arbitrary programming languages.  
* **FR-2.2:** The system shall resolve, fetch, and lock dependency trees into deterministic, cryptographically hashed closures natively, independent of legacy package managers.  
* **FR-2.3:** The graphical interface shall allow users to manually explicitly include, exclude, or pin specific dependency versions prior to executing a build.

### **3\. Native Sandboxing & Daemonless Execution**

* **FR-3.1:** The application shall provide process-level isolation natively on Windows (Job Objects/WSL2), macOS (Seatbelt profiles), and Linux (User Namespaces/chroot) without requiring a global background container daemon or nested Linux virtual machine.  
* **FR-3.2:** The sandbox shall mount the cryptographically locked dependency closure as a read-only volume, ensuring the application cannot mutate its underlying runtime environment.  
* **FR-3.3:** The execution environment shall selectively map host filesystems (the active project directory) into the isolated sandbox with configurable read/write boundaries.

### **4\. Granular Packaging & Offline Artifact Export**

* **FR-4.1:** The platform shall provide a selectable packaging interface with three distinct, natively compiled build targets:  
  * *Target A:* Package dependencies only (Base runtime environment).  
  * *Target B:* Package repository code only (Application layer).  
  * *Target C:* Package repository code combined with locked dependencies (Unified distribution).  
* **FR-4.2:** Using embedded compression libraries (e.g., `tar`, `zstd`), the system shall export built environments into self-contained binary archives optimized for physical transfer.  
* **FR-4.3:** The system shall support importing, verifying, and mounting these archives on air-gapped target machines with zero internet access required.

### **5\. Localized Workspace Supervisor & AI Agent Architecture**

* **FR-5.1:** The engine shall spawn an optional, localized **Workspace Supervisor** thread that lives only for the duration of the active project session. It shall terminate immediately upon workspace closure.  
* **FR-5.2 (Services & Sockets):** The Supervisor shall manage collateral stack services (e.g., local Postgres, Redis) and expose localized API sockets for IDE integration (VS Code, JetBrains).  
* **FR-5.3 (AI Sandbox Constraints):** When running AI agents, the Supervisor shall isolate them in a restrictive sub-sandbox featuring an ephemeral in-memory file system (`tmpfs`) that wipes entirely upon termination.  
* **FR-5.4 (Network Proxies):** The Supervisor shall intercept AI agent outbound network calls, strictly enforcing developer-defined domain whitelists to prevent unauthorized data exfiltration.

### **6\. Unified Graphical User Interface (GUI)**

* **FR-6.1:** The platform shall provide a cross-platform desktop application offering a single-pane-of-glass interface for dependency inspection, agent monitoring, and sandbox controls.  
* **FR-6.2:** The GUI shall provide real-time status telemetry on CPU/RAM allocation, build progress, and Supervisor event loops.

