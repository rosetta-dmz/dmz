use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use tar::Builder as TarBuilder;
use tempfile::tempdir;

use dmz_core::archive::exporter::AirGapManifest;
use dmz_core::archive::{AirGapExporter, AirGapImporter, ImportConfig};
use dmz_core::ecosystem::EcosystemRegistry;
use dmz_core::install::{InstallConfig, InstallerEngine};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::toolchain::{
    DiscoveredToolchain, ToolchainArchivePackager, ToolchainEcosystem, ToolchainPackager,
};

#[test]
fn test_hermetic_end_to_end_complete_codebase_bundle_import_and_stripped_path_verification() {
    let temp = tempdir().unwrap();
    let src_ws = temp.path().join("complete_polyglot_src");
    fs::create_dir_all(src_ws.join("src")).unwrap();

    // Write complete polyglot workspace manifests (Rust + Python + Node + Go + CMake)
    fs::write(
        src_ws.join("Cargo.toml"),
        r#"[package]
name = "hermetic-app"
version = "0.1.0"
edition = "2021"

[dependencies]
mini-util = "0.1.0"
"#,
    )
    .unwrap();
    fs::write(src_ws.join("src/lib.rs"), "pub fn check() -> u32 { 42 }\n").unwrap();
    fs::write(
        src_ws.join("pyproject.toml"),
        "[project]\nname = \"py-app\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(src_ws.join("app.py"), "def run():\n    return 'ok'\n").unwrap();
    fs::write(
        src_ws.join("package.json"),
        r#"{"name": "node-app", "version": "1.0.0", "dependencies": {"js-util": "1.0.0"}}"#,
    )
    .unwrap();
    fs::write(
        src_ws.join("go.mod"),
        "module example.com/hermetic\n\ngo 1.22\n",
    )
    .unwrap();
    fs::write(src_ws.join("main.go"), "package main\nfunc main() {}\n").unwrap();
    fs::write(
        src_ws.join("CMakeLists.txt"),
        "cmake_minimum_required(VERSION 3.10)\nproject(hermetic_native C CXX)\n",
    )
    .unwrap();

    // Build real SHA-256 verified store artifacts for Cargo (`mini-util`) and npm (`js-util`)
    let store_dir = src_ws.join(".dmz/store");
    fs::create_dir_all(store_dir.join("mini-util")).unwrap();
    let crate_archive_path = store_dir.join("mini-util/mini-util-0.1.0.archive");
    {
        let file = fs::File::create(&crate_archive_path).unwrap();
        let enc = GzEncoder::new(file, Compression::default());
        let mut tar = TarBuilder::new(enc);
        let toml_bytes =
            b"[package]\nname = \"mini-util\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";
        let mut h1 = tar::Header::new_gnu();
        h1.set_size(toml_bytes.len() as u64);
        h1.set_mode(0o644);
        h1.set_cksum();
        tar.append_data(&mut h1, "mini-util-0.1.0/Cargo.toml", &toml_bytes[..])
            .unwrap();
        let lib_bytes = b"pub fn val() -> u32 { 1 }\n";
        let mut h2 = tar::Header::new_gnu();
        h2.set_size(lib_bytes.len() as u64);
        h2.set_mode(0o644);
        h2.set_cksum();
        tar.append_data(&mut h2, "mini-util-0.1.0/src/lib.rs", &lib_bytes[..])
            .unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }
    let crate_sha = format!(
        "{:x}",
        Sha256::digest(fs::read(&crate_archive_path).unwrap())
    );

    fs::create_dir_all(store_dir.join("js-util")).unwrap();
    let npm_archive_path = store_dir.join("js-util/js-util-1.0.0.archive");
    {
        let file = fs::File::create(&npm_archive_path).unwrap();
        let enc = GzEncoder::new(file, Compression::default());
        let mut tar = TarBuilder::new(enc);
        let pkg_json = br#"{"name": "js-util", "version": "1.0.0", "main": "index.js"}"#;
        let mut h1 = tar::Header::new_gnu();
        h1.set_size(pkg_json.len() as u64);
        h1.set_mode(0o644);
        h1.set_cksum();
        tar.append_data(&mut h1, "package/package.json", &pkg_json[..])
            .unwrap();
        let idx_js = b"module.exports = 42;\n";
        let mut h2 = tar::Header::new_gnu();
        h2.set_size(idx_js.len() as u64);
        h2.set_mode(0o644);
        h2.set_cksum();
        tar.append_data(&mut h2, "package/index.js", &idx_js[..])
            .unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }
    let npm_sha = format!("{:x}", Sha256::digest(fs::read(&npm_archive_path).unwrap()));

    let lockfile = DmzLockfile::new(vec![
        LockedPackage {
            name: "mini-util".into(),
            version: "0.1.0".into(),
            ecosystem: Ecosystem::Cargo,
            sha256: crate_sha,
            source_url: None,
            dependencies: vec![],
            member_id: None,
        },
        LockedPackage {
            name: "js-util".into(),
            version: "1.0.0".into(),
            ecosystem: Ecosystem::Npm,
            sha256: npm_sha,
            source_url: None,
            dependencies: vec![],
            member_id: None,
        },
    ]);
    lockfile.save(&src_ws.join("dmz.lock")).unwrap();

    // Create self-contained toolchain binaries for the declared ecosystems
    // (Rust, Python, Node, Go, CMake, Cc, Ninja) that validate offline flags and workspace state
    let tc_stage = temp.path().join("tc_stage");
    let make_script = |dir: &Path, name: &str, body: &str| -> (PathBuf, String) {
        fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\nset -e\n{}\n", body)).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o755));
        }
        (p, format!("bin/{}", name))
    };

    let discovered_toolchains = vec![
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Rust,
            version: "rustc 1.80.0".into(),
            root_dir: tc_stage.join("rust"),
            bin_files: vec![
                make_script(
                    &tc_stage.join("rust/bin"),
                    "cargo",
                    "test \"$1\" = \"check\" && test \"$2\" = \"--offline\" && test -f .cargo/vendor/mini-util-0.1.0/Cargo.toml",
                ),
                make_script(&tc_stage.join("rust/bin"), "rustc", "echo rustc 1.80.0"),
            ],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Python,
            version: "Python 3.12.0".into(),
            root_dir: tc_stage.join("python"),
            bin_files: vec![make_script(
                &tc_stage.join("python/bin"),
                "python3",
                "test \"$1\" = \"-m\" && test \"$2\" = \"compileall\" && test -f app.py",
            )],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Node,
            version: "v20.11.0".into(),
            root_dir: tc_stage.join("node"),
            bin_files: vec![
                make_script(&tc_stage.join("node/bin"), "node", "echo v20.11.0"),
                make_script(
                    &tc_stage.join("node/bin"),
                    "npm",
                    "test \"$1\" = \"ls\" && test \"$2\" = \"--offline\" && test -f node_modules/js-util/package.json",
                ),
            ],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Go,
            version: "go1.22.0".into(),
            root_dir: tc_stage.join("go"),
            bin_files: vec![make_script(
                &tc_stage.join("go/bin"),
                "go",
                "test \"$1\" = \"build\" && test -f main.go",
            )],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::CMake,
            version: "cmake 3.28.0".into(),
            root_dir: tc_stage.join("cmake"),
            bin_files: vec![make_script(
                &tc_stage.join("cmake/bin"),
                "cmake",
                "test -f \"$CMAKE_TOOLCHAIN_FILE\"",
            )],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Cc,
            version: "clang 17.0.0".into(),
            root_dir: tc_stage.join("cc"),
            bin_files: vec![
                make_script(&tc_stage.join("cc/bin"), "clang", "echo clang 17.0.0"),
                make_script(&tc_stage.join("cc/bin"), "clang++", "echo clang++ 17.0.0"),
            ],
            lib_files: vec![],
        },
        DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Ninja,
            version: "1.11.1".into(),
            root_dir: tc_stage.join("ninja"),
            bin_files: vec![make_script(
                &tc_stage.join("ninja/bin"),
                "ninja",
                "echo 1.11.1",
            )],
            lib_files: vec![],
        },
    ];

    // Package complete bundle (.tar.zst) with source, store, lockfile, and discovered toolchains + .sha256 sidecar
    let bundle_path = temp.path().join("dist/complete_hermetic_bundle.tar.zst");
    fs::create_dir_all(bundle_path.parent().unwrap()).unwrap();
    {
        let file = fs::File::create(&bundle_path).unwrap();
        let zstd_enc = zstd::stream::write::Encoder::new(file, 3).unwrap();
        let mut tar = TarBuilder::new(zstd_enc);

        tar.append_path_with_name(src_ws.join("dmz.lock"), "dmz.lock")
            .unwrap();
        tar.append_path_with_name(
            &crate_archive_path,
            "store/mini-util/mini-util-0.1.0.archive",
        )
        .unwrap();
        tar.append_path_with_name(&npm_archive_path, "store/js-util/js-util-1.0.0.archive")
            .unwrap();
        for f in &[
            "Cargo.toml",
            "src/lib.rs",
            "pyproject.toml",
            "app.py",
            "package.json",
            "go.mod",
            "main.go",
            "CMakeLists.txt",
        ] {
            tar.append_path_with_name(src_ws.join(f), f).unwrap();
        }

        let tc_entries =
            ToolchainArchivePackager::package(&mut tar, &discovered_toolchains).unwrap();
        let manifest = AirGapManifest {
            dmz_version: "1.2.7".into(),
            target_platform: None,
            vcs: None,
            delta_base: None,
            has_git_history: false,
            toolchains: tc_entries,
            workspace_project: None,
            closure_signature: lockfile.closure_signature.clone(),
            package_count: 2,
            created_at_utc: 0,
            archive_sha256: String::new(),
        };
        let manifest_json = serde_json::to_string_pretty(&manifest).unwrap();
        let mut mhdr = tar::Header::new_gnu();
        mhdr.set_size(manifest_json.len() as u64);
        mhdr.set_mode(0o644);
        mhdr.set_cksum();
        tar.append_data(&mut mhdr, "airgap_manifest.json", manifest_json.as_bytes())
            .unwrap();

        let zstd_enc = tar.into_inner().unwrap();
        zstd_enc.finish().unwrap();
    }
    let archive_sha = AirGapExporter::calculate_file_sha256(&bundle_path).unwrap();
    fs::write(
        format!("{}.sha256", bundle_path.display()),
        format!("{}  complete_hermetic_bundle.tar.zst\n", archive_sha),
    )
    .unwrap();

    // Import onto a clean target machine directory & materialize offline dependencies
    let target_ws = temp.path().join("airgap_target_machine");
    let target_store = target_ws.join(".dmz/store");
    let import_res = AirGapImporter::import(&ImportConfig {
        archive_path: bundle_path,
        target_store_dir: target_store.clone(),
        target_workspace_dir: target_ws.clone(),
        verify_closure_hash: true,
    })
    .expect("Complete bundle import must succeed");
    assert!(import_res.has_toolchains);
    assert_eq!(import_res.imported_packages, 2);

    let installed_count = InstallerEngine::install(&InstallConfig {
        workspace_path: target_ws.clone(),
        lockfile_path: target_ws.join("dmz.lock"),
        store_dir: target_store,
    })
    .expect("Offline dependency materialization must succeed");
    assert_eq!(installed_count, 2);

    // Execute post-import verification across ALL active ecosystems + native build tools
    // under a strictly stripped PATH (ONLY .dmz/toolchains/*/bin and /bin)
    #[cfg(unix)]
    {
        let mut hermetic_env = HashMap::new();
        // Start with ONLY /bin so no host cargo, python3, npm, go, or cmake can leak from /usr/local or /opt/homebrew
        hermetic_env.insert("PATH".to_string(), "/bin".to_string());
        ToolchainPackager::inject_toolchain_paths(&target_ws, &mut hermetic_env);

        let registry = EcosystemRegistry::new();
        let active_handlers = registry.discover_active(&target_ws);
        assert_eq!(
            active_handlers.len(),
            4,
            "Must discover Rust, npm, Go, and Python handlers"
        );

        for handler in active_handlers {
            let cmd = handler
                .verification_command(&target_ws)
                .expect("Handler must provide verification command");
            let status = std::process::Command::new(&cmd.program)
                .args(&cmd.args)
                .current_dir(&target_ws)
                .env_clear()
                .envs(&hermetic_env)
                .status()
                .unwrap_or_else(|e| {
                    panic!(
                        "Failed to spawn bundled verification binary '{}' for ecosystem '{}': {}",
                        cmd.program,
                        handler.name(),
                        e
                    )
                });
            assert!(
                status.success(),
                "Hermetic offline verification failed for ecosystem '{}' ({} {:?})",
                handler.name(),
                cmd.program,
                cmd.args
            );
        }

        // Verify bundled native build tools (cmake, clang, ninja) execute from stripped PATH
        // and that CMAKE_TOOLCHAIN_FILE points to the generated dmz.toolchain.cmake
        for native_tool in &["cmake", "clang", "ninja"] {
            let status = std::process::Command::new(native_tool)
                .current_dir(&target_ws)
                .env_clear()
                .envs(&hermetic_env)
                .status()
                .unwrap_or_else(|e| {
                    panic!(
                        "Failed to execute bundled native build tool '{}' under stripped PATH: {}",
                        native_tool, e
                    )
                });
            assert!(
                status.success(),
                "Bundled native build tool '{}' returned non-zero under stripped PATH",
                native_tool
            );
        }
    }
}
