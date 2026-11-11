pub mod atomic;
pub mod config;
pub mod diagnostics;
pub mod discovery;
pub mod env;
pub mod inference;
pub mod layout;
pub mod schema;
pub mod socket;

pub use atomic::{atomic_write_file, atomic_write_file_with_mode, DEFAULT_FILE_MODE};
pub use config::{
    build_member_env, canonical_virtual_path, cleanup_stale_socket, detect_default_command,
    discover_or_from_lockfile, discover_workspace_manifest, host_target_triple,
    populate_detected_defaults, resolve_command, resolve_executable_in_env, resolve_member_layouts,
    safe_socket_path, sanitize_folder_name, IpcBridgeConfig, MemberRole, ResolvedMemberLayout,
    WorkspaceMember, WorkspaceProjectConfig,
};
pub use diagnostics::{analyze_sidecar_abi, check_socket_path_length, SidecarAbiReport};
