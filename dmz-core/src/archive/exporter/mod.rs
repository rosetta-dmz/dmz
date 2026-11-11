pub mod engine;
pub mod hashing;
pub mod multi_workspace;
pub mod packaging;
pub mod types;

pub use engine::AirGapExporter;
pub use hashing::{calculate_file_sha256, HashingWriter};
pub use packaging::resolve_deterministic_epoch;
pub use types::{AirGapManifest, DeltaBaseInfo, ExportConfig, ExportError, ExportMode};

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn test_export_config_defaults() {
        let config = ExportConfig::default();
        assert_eq!(config.mode, ExportMode::Unified);
        assert_eq!(config.zstd_level, 3);
        assert!(!config.with_git_history);
        assert!(!config.with_toolchain);
    }

    #[test]
    fn test_hashing_writer_accuracy() {
        let mut buffer = Vec::new();
        let payload = b"Hello, hermetic DMZ air-gap packaging!";
        let mut writer = HashingWriter::new(&mut buffer);

        writer.write_all(payload).unwrap();
        assert_eq!(writer.bytes_written(), payload.len() as u64);

        let (hash, written) = writer.finalize().unwrap();
        assert_eq!(written, payload.len() as u64);

        let expected_hash = format!("{:x}", Sha256::digest(payload));
        assert_eq!(hash, expected_hash);
    }

    #[test]
    fn test_calculate_file_sha256() {
        let temp = tempdir().unwrap();
        let file_path = temp.path().join("sample.txt");
        let content = b"Cryptographic verification payload";
        std::fs::write(&file_path, content).unwrap();

        let computed_hash = AirGapExporter::calculate_file_sha256(&file_path).unwrap();
        let expected_hash = format!("{:x}", Sha256::digest(content));
        assert_eq!(computed_hash, expected_hash);
    }

    #[test]
    fn test_deterministic_epoch_resolution() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        // Without git or SOURCE_DATE_EPOCH, falls back to Bazel/DOS epoch (1980-01-01)
        let epoch = resolve_deterministic_epoch(ws, None);
        assert!(epoch > 0);
    }

    #[test]
    fn test_delta_base_info_serde() {
        let delta = DeltaBaseInfo {
            base_commit: Some("abc1234".to_string()),
            base_archive_sha256: Some("deadbeef".to_string()),
            base_closure_signature: None,
        };
        let json = serde_json::to_string(&delta).unwrap();
        assert!(json.contains("abc1234"));
        assert!(!json.contains("base_closure_signature"));

        let deserialized: DeltaBaseInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.base_commit.as_deref(), Some("abc1234"));
    }
}

