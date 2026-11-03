pub mod config;
pub mod diagnostics;

pub use config::{
    cleanup_stale_socket, safe_socket_path, IpcBridgeConfig, MemberRole, WorkspaceMember,
    WorkspaceProjectConfig,
};
pub use diagnostics::{analyze_sidecar_abi, check_socket_path_length, SidecarAbiReport};
