pub mod bundler;
pub mod cargo;
pub mod composer;
pub mod go;
pub mod maven;
pub mod npm;
pub mod pnpm;
pub mod pypi;

pub use bundler::BundlerMapper;
pub use cargo::CargoMapper;
pub use composer::ComposerMapper;
pub use go::GoMapper;
pub use maven::MavenMapper;
pub use npm::NpmMapper;
pub use pnpm::PnpmMapper;
pub use pypi::PyPiMapper;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_pnpm_ensure_modules_yaml() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        // Enforce creation of node_modules/.modules.yaml with isolated linker when .pnpm store directory exists
        fs::create_dir_all(ws.join("node_modules/.pnpm")).unwrap();
        PnpmMapper::ensure_modules_yaml(ws);

        let yaml_path = ws.join("node_modules/.modules.yaml");
        assert!(yaml_path.is_file());
        let content = fs::read_to_string(&yaml_path).unwrap();
        assert!(content.contains("nodeLinker: isolated"));
        assert!(content.contains("packageManager: pnpm"));
        assert!(content.contains("virtualStoreDir: .pnpm"));

        // Enforce that NpmMapper hydration materializes both .modules.yaml and CLI shims (e.g. tauri)
        let temp2 = tempdir().unwrap();
        let ws2 = temp2.path();
        fs::write(
            ws2.join("package.json"),
            r#"{"name": "test", "devDependencies": {"@tauri-apps/cli": "^2.0.0"}, "scripts": {"tauri": "tauri"}}"#,
        )
        .unwrap();
        NpmMapper::hydrate_workspace_bin_shims(ws2);
        assert!(ws2.join("node_modules/.modules.yaml").is_file());
        assert!(ws2.join("node_modules/.bin/tauri").is_file());
    }

    #[test]
    fn test_maven_mapper_copy() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let jar_path = temp.path().join("dummy.jar");
        fs::write(&jar_path, b"PK0304").unwrap();

        let pkg = crate::resolver::lockfile::LockedPackage {
            name: "org.example/test-pkg".into(),
            version: "1.0.0".into(),
            ecosystem: crate::resolver::manifest::types::Ecosystem::MavenGradle,
            sha256: "dummy".into(),
            source_url: Some("dummy".into()),
            dependencies: vec![],
            member_id: None,
        };

        MavenMapper::map_package(ws, &pkg, &jar_path).unwrap();
        let target = ws.join(".m2/repository/org.example/test-pkg/org.example/test-pkg-1.0.0.jar");
        assert!(target.exists());
        assert_eq!(fs::read(&target).unwrap(), b"PK0304");
    }

    #[test]
    fn test_pypi_version_detection_default() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let ver = PyPiMapper::detect_python_version(ws);
        assert_eq!(ver, "python3.11");
    }

    #[test]
    fn test_pypi_version_detection_existing_venv() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let venv_lib = ws.join(".venv/lib/python3.12");
        fs::create_dir_all(&venv_lib).unwrap();
        let ver = PyPiMapper::detect_python_version(ws);
        assert_eq!(ver, "python3.12");
    }

    #[test]
    fn test_pypi_hydrate_venv_environment() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let root_venv = ws.join(".venv");
        PyPiMapper::hydrate_venv_environment(ws, &root_venv, "python3.11");

        let cfg = root_venv.join("pyvenv.cfg");
        assert!(cfg.is_file());
        let content = fs::read_to_string(cfg).unwrap();
        assert!(content.contains("version = 3.11.0"));

        #[cfg(unix)]
        {
            assert!(root_venv.join("bin/python").is_file());
            assert!(root_venv.join("bin/python3").is_file());
            assert!(root_venv.join("bin/activate").is_file());
        }
        #[cfg(windows)]
        {
            assert!(root_venv.join("Scripts/python.bat").is_file());
        }
    }

    fn create_test_tar_gz(filename: &str, content: &[u8]) -> Vec<u8> {
        let mut tar_builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar_builder
            .append_data(&mut header, filename, content)
            .unwrap();
        let tar_bytes = tar_builder.into_inner().unwrap();

        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn test_go_mapper() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let archive_data = create_test_tar_gz("main.go", b"package main");
        let archive_path = temp.path().join("go-pkg.tar.gz");
        fs::write(&archive_path, &archive_data).unwrap();

        let pkg = crate::resolver::lockfile::LockedPackage {
            name: "golang.org/x/tools".into(),
            version: "v0.1.0".into(),
            ecosystem: crate::resolver::manifest::types::Ecosystem::Go,
            sha256: "dummy".into(),
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };

        GoMapper::map_package(ws, &pkg, &archive_path).unwrap();
        assert!(ws.join("vendor/golang.org/x/tools/main.go").is_file());
        let modules_txt = ws.join("vendor/modules.txt");
        assert!(modules_txt.is_file());
        let content = fs::read_to_string(modules_txt).unwrap();
        assert!(content.contains("golang.org/x/tools v0.1.0"));
    }

    #[test]
    fn test_composer_mapper() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let archive_data = create_test_tar_gz("composer.json", b"{\"name\": \"psr/log\"}");
        let archive_path = temp.path().join("psr-log.tar.gz");
        fs::write(&archive_path, &archive_data).unwrap();

        let pkg = crate::resolver::lockfile::LockedPackage {
            name: "psr/log".into(),
            version: "1.1.4".into(),
            ecosystem: crate::resolver::manifest::types::Ecosystem::Composer,
            sha256: "dummy".into(),
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };

        ComposerMapper::map_package(ws, &pkg, &archive_path).unwrap();
        assert!(ws.join("vendor/psr/log/composer.json").is_file());
    }

    #[test]
    fn test_bundler_mapper() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let archive_data = create_test_tar_gz("rake.gemspec", b"# dummy gemspec");
        let archive_path = temp.path().join("rake.tar.gz");
        fs::write(&archive_path, &archive_data).unwrap();

        let pkg = crate::resolver::lockfile::LockedPackage {
            name: "rake".into(),
            version: "13.0.6".into(),
            ecosystem: crate::resolver::manifest::types::Ecosystem::Bundler,
            sha256: "dummy".into(),
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };

        BundlerMapper::map_package(ws, &pkg, &archive_path).unwrap();
        assert!(ws.join("vendor/bundle/rake/rake.gemspec").is_file());
        let config = ws.join(".bundle/config");
        assert!(config.is_file());
        let content = fs::read_to_string(config).unwrap();
        assert!(content.contains("BUNDLE_PATH: \"vendor/bundle\""));
    }
}
