// DMZ Hermetic Air-Gap Archive Subsystem

pub mod exporter;
pub mod importer;

pub use exporter::{
    calculate_file_sha256, AirGapExporter, AirGapManifest, DeltaBaseInfo, ExportConfig,
    ExportError, ExportMode, HashingWriter,
};
pub use importer::{
    hydrate_offline_dependencies, relative_path_between, AirGapImporter, ImportConfig, ImportError,
    ImportResult,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_archive_facade_config_defaults() {
        let export_cfg = ExportConfig::default();
        assert_eq!(export_cfg.mode, ExportMode::Unified);
        assert_eq!(export_cfg.zstd_level, 3);

        let import_cfg = ImportConfig::default();
        assert!(import_cfg.verify_closure_hash);
        assert_eq!(
            import_cfg.target_workspace_dir.to_string_lossy(),
            "workspace"
        );
    }

    #[test]
    fn test_archive_facade_error_traits() {
        let exp_err = ExportError::ArchiveError("test export error".into());
        assert!(exp_err.to_string().contains("test export error"));
        let _: &dyn std::error::Error = &exp_err;

        let imp_err = ImportError::ExtractionError("test import error".into());
        assert!(imp_err.to_string().contains("test import error"));
        let _: &dyn std::error::Error = &imp_err;
    }
}