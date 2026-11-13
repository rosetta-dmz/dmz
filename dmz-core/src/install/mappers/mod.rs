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
        let target = ws.join(".m2/repository/org/example/test-pkg/1.0.0/test-pkg-1.0.0.jar");
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
        PyPiMapper::hydrate_venv_environment(ws, &root_venv, "python3.11").unwrap();

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

    #[test]
    fn test_cargo_mapper_per_file_checksums_and_idempotent_config() {
        use sha2::{Digest, Sha256};

        let temp = tempdir().unwrap();
        let ws = temp.path();
        let cargo_toml = b"[package]\nname = \"mini\"\nversion = \"0.1.0\"\n";
        let lib_rs = b"pub fn hello() -> &'static str { \"hi\" }\n";

        let mut tar_builder = tar::Builder::new(Vec::new());
        for (rel_path, content) in &[
            ("mini-0.1.0/Cargo.toml", &cargo_toml[..]),
            ("mini-0.1.0/src/lib.rs", &lib_rs[..]),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar_builder
                .append_data(&mut header, rel_path, *content)
                .unwrap();
        }
        let tar_bytes = tar_builder.into_inner().unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &tar_bytes).unwrap();
        let archive_data = encoder.finish().unwrap();

        let archive_path = temp.path().join("mini-0.1.0.crate");
        fs::write(&archive_path, &archive_data).unwrap();
        let pkg_sha = crate::archive::format_sha256_hex(&Sha256::digest(&archive_data));

        let pkg = crate::resolver::lockfile::LockedPackage {
            name: "mini".into(),
            version: "0.1.0".into(),
            ecosystem: crate::resolver::manifest::types::Ecosystem::Cargo,
            sha256: pkg_sha.clone(),
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };

        CargoMapper::map_package(ws, &pkg, &archive_path).unwrap();
        CargoMapper::map_package(ws, &pkg, &archive_path).unwrap();

        let checksum_file = ws.join(".cargo/vendor/mini-0.1.0/.cargo-checksum.json");
        assert!(checksum_file.is_file());
        let checksum_json: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&checksum_file).unwrap()).unwrap();
        assert_eq!(checksum_json["package"], pkg_sha);
        assert_eq!(
            checksum_json["files"]["Cargo.toml"],
            crate::archive::format_sha256_hex(&Sha256::digest(cargo_toml))
        );
        assert_eq!(
            checksum_json["files"]["src/lib.rs"],
            crate::archive::format_sha256_hex(&Sha256::digest(lib_rs))
        );

        let config_toml = fs::read_to_string(ws.join(".cargo/config.toml")).unwrap();
        assert_eq!(config_toml.matches("[source.vendored-sources]").count(), 1);
    }

    #[test]
    fn test_go_mapper_strips_module_prefix_and_dedups_modules_txt() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let zip_path = temp.path().join("mod.zip");
        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("github.com/acme/lib@v1.2.3/lib.go", opts)
                .unwrap();
            std::io::Write::write_all(&mut zip, b"package lib\n").unwrap();
            zip.finish().unwrap();
        }

        let pkg = crate::resolver::lockfile::LockedPackage {
            name: "github.com/acme/lib".into(),
            version: "v1.2.3".into(),
            ecosystem: crate::resolver::manifest::types::Ecosystem::Go,
            sha256: "abc".into(),
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };

        GoMapper::map_package(ws, &pkg, &zip_path).unwrap();
        GoMapper::map_package(ws, &pkg, &zip_path).unwrap();

        assert!(ws.join("vendor/github.com/acme/lib/lib.go").is_file());
        let modules_txt = fs::read_to_string(ws.join("vendor/modules.txt")).unwrap();
        assert_eq!(
            modules_txt.matches("# github.com/acme/lib v1.2.3").count(),
            1
        );
        assert!(modules_txt.contains("## explicit"));
    }

    #[test]
    fn test_maven_mapper_populates_m2_repository_layout() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let jar_path = temp.path().join("gson.jar");
        fs::write(&jar_path, b"PK\x03\x04mock-jar").unwrap();

        let pkg = crate::resolver::lockfile::LockedPackage {
            name: "com.google.code.gson:gson".into(),
            version: "2.10.1".into(),
            ecosystem: crate::resolver::manifest::types::Ecosystem::MavenGradle,
            sha256: "abc".into(),
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };

        MavenMapper::map_package(ws, &pkg, &jar_path).unwrap();
        let expected_jar =
            ws.join(".m2/repository/com/google/code/gson/gson/2.10.1/gson-2.10.1.jar");
        assert!(expected_jar.is_file());
        assert_eq!(fs::read(&expected_jar).unwrap(), b"PK\x03\x04mock-jar");
    }
}
