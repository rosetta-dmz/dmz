# For CLI

[] This folder is for the future extraction of `command.rs`(currently 1007 lines long) and will follow SOLID principles. Commands will be organized into 12 distinct files as follows:

  resolve    Scan workspace manifests and generate a deterministic dmz.lock closure
  fetch      Fetch and cache locked dependencies into the secure store
  install    Install locked dependencies offline from store into project layouts
  build      Build deterministic package artifacts (Target A: Deps, Target B: App, Target C: Unified)
  sandbox    Execute a command inside a native daemonless OS sandbox
  airgap     Manage offline, air-gapped package bundles
  bundle     One-shot resolution, dependency fetching, and export into an air-gap bundle (.tar.zst)
  import     Unpack, verify, and restore an air-gap bundle into local store
  shell      Open an interactive shell inside the workspace (sandboxed by default)
  init       Initialize a project workspace for DMZ capture
  ecosystem  Inspect and manage multi-ecosystem workspace stacks (Rust, Node, Go, Python, etc.)
  help       Print this message or the help of the given subcommand(s)