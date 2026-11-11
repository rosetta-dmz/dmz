// Workspace configuration facade and re-export module.
// Re-exports configuration schemas, ecosystem inference, virtual layouts,
// process environments, manifest discovery, and socket lifecycle helpers.

pub use super::discovery::{discover_or_from_lockfile, discover_workspace_manifest};
pub use super::env::{
    build_member_env, host_target_triple, inject_node_env, inject_offline_env, inject_python_env,
    resolve_executable_in_env,
};
pub use super::inference::{
    detect_default_command, detect_go_command, detect_node_command, detect_python_command,
    detect_rust_command, populate_detected_defaults, resolve_command,
};
pub use super::layout::{canonical_virtual_path, resolve_member_layouts, sanitize_folder_name};
pub use super::schema::{
    IpcBridgeConfig, MemberRole, ResolvedMemberLayout, WorkspaceMember, WorkspaceProjectConfig,
};
pub use super::socket::{cleanup_stale_socket, safe_socket_path};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_facade_exports_compile_and_resolve() {
        let cfg = WorkspaceProjectConfig::default();
        assert_eq!(cfg.name, "default");

        let triple = host_target_triple();
        assert!(!triple.is_empty());

        let member = WorkspaceMember::new("test", ".", MemberRole::Primary);
        let virt_path = canonical_virtual_path(&member);
        assert_eq!(virt_path, std::path::PathBuf::from("workspaces/test"));
    }
}
