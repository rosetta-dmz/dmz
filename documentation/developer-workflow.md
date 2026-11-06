### **Step 1: Set Up or Navigate to an External Test Project**

Open a new terminal window **outside** of your DMZ repository folder and navigate to any existing project (or create a quick test project with a standard manifest like `package.json`, `Cargo.toml`, or `requirements.txt`).

For example, let's use a sample Node.js or Rust project directory:

```bash
cd /path/to/my-external-app
```

---

### **Step 2: Run Dependency Resolution**

Run the resolver to scan the project manifests, resolve dependency closures, and generate a deterministic `dmz.lock` file:

```bash
dmz resolve --workspace . --output dmz.lock
```

* **Expected Result:** A clean `dmz.lock` file is created in your project root, logging the resolved dependency tree and cryptographic hashes.

---

### **Step 3: Export the Air-Gap Bundle**

Package your project workspace, locked dependencies, and cached store items into a compressed portable archive (`.tar.zst`):

```bash
dmz airgap export --lockfile dmz.lock --store .dmz/store --output airgap_bundle.tar.zst --zstd_level 3
```

* **Expected Result:** A compressed archive `airgap_bundle.tar.zst` is generated in your project root, containing everything required to run offline.

---

### **Step 4: Simulate the Air-Gapped Environment**

To verify the air-gapped target behavior, simulate moving to a completely offline machine by clearing your local cache or copying `airgap_bundle.tar.zst` into a brand-new, isolated directory:

```bash
mkdir -p /tmp/airgapped-target/my-app
cp airgap_bundle.tar.zst /tmp/airgapped-target/
cd /tmp/airgapped-target/
```

---

### **Step 5: Import, Install Offline Dependencies, and Run in the Secure Sandbox**

On your target environment, unpack and import the bundle into the local secure store, materialize the dependencies offline into standard project layouts, and execute your build or run command inside DMZ's daemonless native OS sandbox:

1. **Import the bundle:**
```bash
dmz airgap import --archive airgap_bundle.tar.zst --store ./offline_store
```

2. **Materialize dependencies offline into the workspace:**
```bash
cd workspace
dmz install --store ../offline_store --lockfile dmz.lock
```

3. **Execute inside the sandbox or run native verification:**
```bash
dmz ecosystem verify
# or execute inside the sandbox:
dmz sandbox run --workspace . --closure ../offline_store -- [your-build-or-run-command]
```

*(Should you run into any host platform or environment issues, append `--diagnose` to output plain-language error classifications and remediation steps).*