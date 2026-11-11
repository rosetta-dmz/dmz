pub mod archive;
pub mod diagnostics;
pub mod ecosystem;
pub mod install;
pub mod packaging;
pub mod resolver;
pub mod toolchain;
pub mod vcs;
pub mod workspace;

pub use resolver::ResolverEngine;
pub use vcs::{BundleInfo, GitProvider, VcsError, VcsProvenance, VcsProvider};
pub use workspace::{
    analyze_sidecar_abi, check_socket_path_length, cleanup_stale_socket, safe_socket_path,
    IpcBridgeConfig, MemberRole, ResolvedMemberLayout, SidecarAbiReport, WorkspaceMember,
    WorkspaceProjectConfig,
};
