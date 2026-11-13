use dmz_core::install::{InstallConfig, InstallerEngine};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::{Ecosystem, ManifestScanner};
use dmz_core::resolver::ResolverEngine;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use tempfile::tempdir;

fn build_go_mod_zip(module_name: &str, version: &str, go_file: &str, source: &[u8]) -> Vec<u8> {
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut cursor);
        let opts = zip::write::SimpleFileOptions::default();
        let entry_path = format!("{}@{}/{}", module_name, version, go_file);
        zip.start_file(entry_path, opts).unwrap();
        zip.write_all(source).unwrap();
        zip.finish().unwrap();
    }
    cursor.into_inner()
}

#[test]
fn test_airgap_offline_install_go_workflow() -> Result<(), Box<dyn std::error::Error>> {
    let tmp_dir = tempdir()?;
    let workspace_path = tmp_dir.path().join("workspace");
    let store_path = tmp_dir.path().join(".dmz/store");
    fs::create_dir_all(&workspace_path)?;
    fs::create_dir_all(&store_path)?;

    fs::write(
        workspace_path.join("go.mod"),
        r#"module example.com/myservice

go 1.22

require (
    golang.org/x/text v0.14.0
    github.com/pkg/errors v0.9.1 // indirect
)
"#,
    )?;
    fs::write(
        workspace_path.join("go.sum"),
        "golang.org/x/text v0.14.0 h1:abc123=\ngolang.org/x/text v0.14.0/go.mod h1:def456=\n",
    )?;
    fs::write(
        workspace_path.join("main.go"),
        "package main\nfunc main() {}\n",
    )?;

    // 1. Manifest scanner + resolver check
    let manifests = ManifestScanner::scan_dir(&workspace_path)?;
    assert_eq!(manifests.len(), 1);
    assert_eq!(manifests[0].ecosystem, Ecosystem::Go);

    let resolved_lock = ResolverEngine::resolve_workspace(&workspace_path)?;
    assert!(resolved_lock
        .packages
        .values()
        .any(|p| p.name == "golang.org/x/text" && p.version == "v0.14.0"));
    assert!(resolved_lock
        .packages
        .values()
        .any(|p| p.name == "github.com/pkg/errors" && p.version == "v0.9.1"));

    // 2. Populate Go module zips into the store with real SHA-256 digests
    let mut packages = BTreeMap::new();
    for (key, pkg) in resolved_lock.packages {
        let safe_name = pkg.name.replace('/', "__");
        let zip_bytes = build_go_mod_zip(
            &pkg.name,
            &pkg.version,
            "lib.go",
            format!("package mod_{}\n", safe_name.replace(['.', '-', '/'], "_")).as_bytes(),
        );
        let pkg_dir = store_path.join(&safe_name);
        fs::create_dir_all(&pkg_dir)?;
        fs::write(
            pkg_dir.join(format!("{}-{}.archive", safe_name, pkg.version)),
            &zip_bytes,
        )?;
        let sha256 = dmz_core::archive::format_sha256_hex(&Sha256::digest(&zip_bytes));
        packages.insert(key, LockedPackage { sha256, ..pkg });
    }

    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
        workspaces: BTreeMap::new(),
        closure_signature: DmzLockfile::compute_closure_hash(&packages),
        packages,
    };
    let lockfile_path = workspace_path.join("dmz.lock");
    lockfile.save(&lockfile_path)?;

    // 3. Offline install
    let config = InstallConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path,
        store_dir: store_path,
    };
    let installed_count = InstallerEngine::install(&config)?;
    assert_eq!(installed_count, 2);

    // 4. Verify prefix-stripped vendor tree and vendor/modules.txt
    assert!(workspace_path
        .join("vendor/golang.org/x/text/lib.go")
        .is_file());
    assert!(workspace_path
        .join("vendor/github.com/pkg/errors/lib.go")
        .is_file());
    let modules_txt = fs::read_to_string(workspace_path.join("vendor/modules.txt"))?;
    assert!(modules_txt.contains("# golang.org/x/text v0.14.0"));
    assert!(modules_txt.contains("# github.com/pkg/errors v0.9.1"));
    assert!(modules_txt.contains("## explicit"));

    Ok(())
}
