pub mod backup;
pub mod engine;
pub mod hydration;
pub mod security;
pub mod types;

pub use engine::AirGapImporter;
pub use hydration::{hydrate_offline_dependencies, relative_path_between};
pub use security::{is_contained_path, is_safe_relative_path, unpack_tar_entry_contained};
pub use types::{ImportConfig, ImportError, ImportResult};

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use tempfile::tempdir;

    #[test]
    fn test_relative_path_between_arbitrary_depths() {
        let temp = tempdir().unwrap();
        let ws = temp.path().join("workspace");
        let store = ws.join(".dmz/store");
        let tauri = ws.join("src-tauri");
        let umbrella_primary = ws.join("workspaces/primary");

        fs::create_dir_all(&store).unwrap();
        fs::create_dir_all(&tauri).unwrap();
        fs::create_dir_all(&umbrella_primary).unwrap();

        // 1. Root workspace to store -> ".dmz/store"
        let rel_root = AirGapImporter::relative_path_between(&ws, &store);
        assert_eq!(rel_root, ".dmz/store");

        // 2. Tauri to store -> "../.dmz/store"
        let rel_tauri = AirGapImporter::relative_path_between(&tauri, &store);
        assert_eq!(rel_tauri, "../.dmz/store");

        // 3. Umbrella member to store -> "../../.dmz/store"
        let rel_umbrella = AirGapImporter::relative_path_between(&umbrella_primary, &store);
        assert_eq!(rel_umbrella, "../../.dmz/store");
    }

    #[test]
    fn test_archive_importer_rejects_path_traversal() {
        let temp = tempdir().unwrap();
        let ws = temp.path().join("ws");
        let store = temp.path().join("store");
        let archive_path = temp.path().join("traversal.tar.zst");

        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&store).unwrap();

        // Build traversal archive containing store/../../escaped.txt
        let file = File::create(&archive_path).unwrap();
        let enc = zstd::stream::Encoder::new(file, 3).unwrap();
        let mut tar = tar::Builder::new(enc);

        let manifest = crate::archive::AirGapManifest {
            dmz_version: "1.2.8".to_string(),
            target_platform: None,
            vcs: None,
            delta_base: None,
            has_git_history: false,
            toolchains: vec![],
            workspace_project: None,
            closure_signature: "mock".to_string(),
            package_count: 1,
            created_at_utc: 0,
            archive_sha256: "mock".to_string(),
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(
            &mut header,
            "airgap_manifest.json",
            manifest_bytes.as_slice(),
        )
        .unwrap();

        let traversal_payload = b"traversal payload";
        let mut traversal_header = tar::Header::new_gnu();
        traversal_header.set_size(traversal_payload.len() as u64);
        traversal_header.set_mode(0o644);
        let name_bytes = b"store/../../escaped.txt";
        traversal_header.as_mut_bytes()[..name_bytes.len()].copy_from_slice(name_bytes);
        traversal_header.set_cksum();
        tar.append(&traversal_header, &traversal_payload[..]).unwrap();

        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap();

        let config = ImportConfig {
            archive_path,
            target_store_dir: store,
            target_workspace_dir: ws,
            verify_closure_hash: false,
        };

        let result = AirGapImporter::import(&config);
        assert!(
            result.is_err(),
            "Import must fail on path traversal attempt"
        );
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("Path traversal attempt detected"),
            "Error message should mention path traversal, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_archive_importer_rejects_symlink_zip_slip() {
        let temp = tempdir().unwrap();
        let ws = temp.path().join("ws");
        let store = temp.path().join("store");
        let archive_path = temp.path().join("traversal_symlink.tar.zst");

        fs::create_dir_all(&ws).unwrap();
        fs::create_dir_all(&store).unwrap();

        let file = File::create(&archive_path).unwrap();
        let enc = zstd::stream::Encoder::new(file, 3).unwrap();
        let mut tar = tar::Builder::new(enc);

        let manifest = crate::archive::AirGapManifest {
            dmz_version: "1.2.8".to_string(),
            target_platform: None,
            vcs: None,
            delta_base: None,
            has_git_history: false,
            toolchains: vec![],
            workspace_project: None,
            closure_signature: "mock".to_string(),
            package_count: 1,
            created_at_utc: 0,
            archive_sha256: "mock".to_string(),
        };
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(
            &mut header,
            "airgap_manifest.json",
            manifest_bytes.as_slice(),
        )
        .unwrap();

        // Traversal symlink: store/traversal_link -> ../../outside
        let mut sym_header = tar::Header::new_gnu();
        sym_header.set_entry_type(tar::EntryType::Symlink);
        sym_header.set_size(0);
        sym_header.set_mode(0o777);
        let link_target = b"../../outside";
        sym_header.as_mut_bytes()[157..157 + link_target.len()].copy_from_slice(link_target);
        let entry_name = b"store/traversal_link";
        sym_header.as_mut_bytes()[..entry_name.len()].copy_from_slice(entry_name);
        sym_header.set_cksum();
        tar.append(&sym_header, std::io::empty()).unwrap();

        let enc = tar.into_inner().unwrap();
        enc.finish().unwrap();

        let config = ImportConfig {
            archive_path,
            target_store_dir: store,
            target_workspace_dir: ws,
            verify_closure_hash: false,
        };

        let result = AirGapImporter::import(&config);
        assert!(
            result.is_err(),
            "Import must fail on symlink traversal attempt"
        );
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("Path traversal attempt detected in link target"),
            "Error should reject link target, got: {}",
            err_msg
        );
    }
}

