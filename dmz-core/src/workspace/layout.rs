// Virtual archive layout resolution and path sanitization.
// Maps workspace members to normalized `workspaces/<folder>` targets
// while defending against path traversal and Zip Slip vulnerabilities.

use super::schema::{MemberRole, ResolvedMemberLayout, WorkspaceMember, WorkspaceProjectConfig};
use std::fs;
use std::path::{Path, PathBuf};

/// Sanitizes a folder name for virtual archive path generation, preventing path traversal attacks.
pub fn sanitize_folder_name(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = cleaned.trim_matches('_');
    if trimmed.is_empty() || trimmed == ".." || trimmed == "." {
        "member".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Maps a workspace member to its canonical virtual sandbox/archive mount path.
pub fn canonical_virtual_path(member: &WorkspaceMember) -> PathBuf {
    let raw_folder = if member.id.is_empty() || member.id == "default" {
        match member.role {
            MemberRole::Primary => "primary",
            MemberRole::Sidecar | MemberRole::Service => "sidecar",
            MemberRole::Member => "member",
        }
    } else {
        member.id.as_str()
    };
    let sanitized = sanitize_folder_name(raw_folder);
    PathBuf::from(format!("workspaces/{}", sanitized))
}

/// Resolves canonical host directories and normalized `workspaces/<folder>` names for all members.
///
/// # Async Runtime Note
/// Performs synchronous host path canonicalization. When invoking from an asynchronous Tokio
/// runtime context, offload via `tokio::task::spawn_blocking`.
pub fn resolve_member_layouts(
    config: &WorkspaceProjectConfig,
    workspace_root: &Path,
    manifest_path: Option<&Path>,
) -> Vec<ResolvedMemberLayout> {
    let primary_raw_name = workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("primary")
        .to_string();

    let base_dir = manifest_path
        .and_then(|p| p.parent())
        .unwrap_or(workspace_root);

    let mut layouts = Vec::with_capacity(config.members.len());

    for member in &config.members {
        let (host_path, folder_name) = if member.role == MemberRole::Primary
            && (member.path == Path::new(".") || member.path.as_os_str().is_empty())
        {
            let folder =
                if !member.id.is_empty() && member.id != "primary" && member.id != "default" {
                    member.id.clone()
                } else {
                    primary_raw_name.clone()
                };
            (workspace_root.to_path_buf(), folder)
        } else {
            let raw_host = if member.path.is_relative() {
                base_dir.join(&member.path)
            } else {
                member.path.clone()
            };
            let host_path = fs::canonicalize(&raw_host).unwrap_or(raw_host);
            let raw_name = host_path
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|n| !n.is_empty())
                .unwrap_or(&member.id)
                .to_string();

            let folder = if !member.id.is_empty()
                && !matches!(
                    member.id.as_str(),
                    "primary" | "sidecar" | "service" | "default"
                ) {
                member.id.clone()
            } else {
                raw_name
            };
            (host_path, folder)
        };

        let sanitized_folder = sanitize_folder_name(&folder_name);

        layouts.push(ResolvedMemberLayout {
            id: member.id.clone(),
            role: member.role,
            host_path,
            archive_path: PathBuf::from(format!("workspaces/{}", sanitized_folder)),
            folder_name: sanitized_folder,
            member: member.clone(),
        });
    }

    // Disambiguate if any non-primary member resolves to the same folder name as the primary member
    let primary_id = config.primary_member().map(|m| m.id.clone());
    let primary_folder = layouts
        .iter()
        .find(|l| Some(&l.id) == primary_id.as_ref())
        .map(|l| l.folder_name.clone())
        .unwrap_or(primary_raw_name);

    for layout in &mut layouts {
        if Some(&layout.id) != primary_id.as_ref() && layout.folder_name == primary_folder {
            layout.folder_name = format!("{}-sidecar", layout.folder_name);
            layout.archive_path = PathBuf::from(format!("workspaces/{}", layout.folder_name));
        }
    }

    layouts
}

impl WorkspaceMember {
    pub fn canonical_virtual_path(&self) -> PathBuf {
        canonical_virtual_path(self)
    }
}

impl WorkspaceProjectConfig {
    pub fn resolve_member_layouts(
        &self,
        workspace_root: &Path,
        manifest_path: Option<&Path>,
    ) -> Vec<ResolvedMemberLayout> {
        resolve_member_layouts(self, workspace_root, manifest_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_member_layouts_sanitizes_traversal() {
        let ws_root = PathBuf::from("/Users/dev/project");
        let config = WorkspaceProjectConfig::new(
            "test-app",
            vec![
                WorkspaceMember::new("primary", ".", MemberRole::Primary),
                WorkspaceMember::new(
                    "../../traversal-test/component",
                    "../sidecar",
                    MemberRole::Sidecar,
                ),
            ],
        );

        let layouts = config.resolve_member_layouts(&ws_root, None);
        assert_eq!(layouts.len(), 2);
        assert_eq!(layouts[0].folder_name, "project");

        // Traversal input ("../../traversal-test/component") must be sanitized to eliminate Zip Slip risk
        let sidecar_layout = &layouts[1];
        assert!(!sidecar_layout.folder_name.contains(".."));
        assert!(!sidecar_layout.folder_name.contains('/'));
        assert_eq!(sidecar_layout.folder_name, "traversal-test_component");
        assert_eq!(
            sidecar_layout.archive_path,
            PathBuf::from("workspaces/traversal-test_component")
        );
    }

    #[test]
    fn test_sanitize_folder_name_edge_cases() {
        assert_eq!(sanitize_folder_name(""), "member");
        assert_eq!(sanitize_folder_name(".."), "member");
        assert_eq!(sanitize_folder_name("..."), "member");
        assert_eq!(sanitize_folder_name("normal-name_123"), "normal-name_123");
        assert_eq!(sanitize_folder_name("@org/pkg"), "org_pkg");
    }

    #[test]
    fn test_canonical_virtual_path_default_roles() {
        let primary = WorkspaceMember::new("default", ".", MemberRole::Primary);
        assert_eq!(canonical_virtual_path(&primary), PathBuf::from("workspaces/primary"));
        let sidecar = WorkspaceMember::new("", ".", MemberRole::Sidecar);
        assert_eq!(canonical_virtual_path(&sidecar), PathBuf::from("workspaces/sidecar"));
    }
}
