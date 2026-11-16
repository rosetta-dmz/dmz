use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tempfile::tempdir;

fn spawn_mock_registry_server<F>(
    responder: F,
) -> (
    RegistryBases,
    String,
    Arc<AtomicBool>,
    std::thread::JoinHandle<()>,
)
where
    F: Fn(&str, &str) -> (&'static str, &'static str, Vec<u8>) + Send + Sync + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let base_url = format!("http://{}", addr);
    let bases = RegistryBases {
        crates_api: format!("{}/crates_api", base_url),
        crates_dl: format!("{}/crates", base_url),
        npm: format!("{}/npm", base_url),
        pypi: format!("{}/pypi", base_url),
        golang: format!("{}/golang", base_url),
    };

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_srv = Arc::clone(&shutdown);
    let base_srv = base_url.clone();

    listener.set_nonblocking(true).unwrap();
    let handle = std::thread::spawn(move || {
        while !shutdown_srv.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buf = [0u8; 4096];
                    let n = stream.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(buf.get(..n).unwrap_or_default());
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");
                    let (status, ctype, body) = responder(path, &base_srv);
                    let header = format!(
                        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        status,
                        ctype,
                        body.len()
                    );
                    let _ = stream.write_all(header.as_bytes());
                    let _ = stream.write_all(&body);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Err(_) => break,
            }
        }
    });

    (bases, base_url, shutdown, handle)
}

fn pypi_json_response(
    base: &str,
    name: &str,
    version: &str,
    requires_dist: &[&str],
    wheel_filenames: &[&str],
) -> (&'static str, &'static str, Vec<u8>) {
    let urls: Vec<_> = wheel_filenames
        .iter()
        .map(|fname| {
            serde_json::json!({
                "filename": fname,
                "url": format!("{}/files/{}", base, fname),
            })
        })
        .collect();
    let payload = serde_json::json!({
        "info": {
            "name": name,
            "version": version,
            "requires_dist": requires_dist,
        },
        "urls": urls,
    });
    (
        "200 OK",
        "application/json",
        payload.to_string().into_bytes(),
    )
}

#[test]
fn test_fetch_dependencies_pypi_transitive_pep508() {
    let (bases, _base_url, shutdown, handle) =
        spawn_mock_registry_server(|path, base| match path {
            "/pypi/charset_normalizer/2/json" | "/pypi/charset_normalizer/json" => {
                pypi_json_response(
                    base,
                    "charset-normalizer",
                    "2.0.0",
                    &[],
                    &["charset_normalizer-2.0.0-py3-none-any.whl"],
                )
            }
            "/files/charset_normalizer-2.0.0-py3-none-any.whl" => (
                "200 OK",
                "application/octet-stream",
                b"PK\x03\x04charset-normalizer-2.0.0".to_vec(),
            ),
            _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
        });

    let temp = tempdir().unwrap();
    let lockfile_path = temp.path().join("dmz.lock");
    let store_path = temp.path().join("store");

    let pkg = LockedPackage {
        name: "charset_normalizer".to_string(),
        version: "2".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: "dummy".to_string(),
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    };
    DmzLockfile::new(vec![pkg]).save(&lockfile_path).unwrap();

    let count =
        DependencyFetcher::fetch_dependencies_with_bases(&lockfile_path, &store_path, &bases);
    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.join();

    assert!(count.is_ok(), "Failed to fetch: {:?}", count.err());
    assert_eq!(count.unwrap(), 1);

    let cached_artifact = store_path
        .join("charset_normalizer")
        .join("charset_normalizer-2.0.0.archive");
    assert!(
        cached_artifact.exists(),
        "Expected cached artifact at {:?}",
        cached_artifact
    );
}

#[test]
fn test_fetch_dependencies_pypi_requests_transitive_closure() {
    let (bases, _base_url, shutdown, handle) =
        spawn_mock_registry_server(|path, base| match path {
            "/pypi/requests/2.31.0/json" | "/pypi/requests/json" => pypi_json_response(
                base,
                "requests",
                "2.31.0",
                &[
                    "charset-normalizer>=2,<4",
                    "idna>=2.5,<4",
                    "urllib3>=1.21.1,<3",
                    "certifi>=2017.4.17",
                ],
                &["requests-2.31.0-py3-none-any.whl"],
            ),
            "/pypi/charset-normalizer/json" | "/pypi/charset_normalizer/json" => {
                pypi_json_response(
                    base,
                    "charset-normalizer",
                    "3.3.2",
                    &[],
                    &["charset_normalizer-3.3.2-py3-none-any.whl"],
                )
            }
            "/pypi/idna/json" => {
                pypi_json_response(base, "idna", "3.7", &[], &["idna-3.7-py3-none-any.whl"])
            }
            "/pypi/urllib3/json" => pypi_json_response(
                base,
                "urllib3",
                "2.2.1",
                &[],
                &["urllib3-2.2.1-py3-none-any.whl"],
            ),
            "/pypi/certifi/json" => pypi_json_response(
                base,
                "certifi",
                "2024.2.2",
                &[],
                &["certifi-2024.2.2-py3-none-any.whl"],
            ),
            p if p.starts_with("/files/") => (
                "200 OK",
                "application/octet-stream",
                b"PK\x03\x04mock-wheel".to_vec(),
            ),
            _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
        });

    let temp = tempdir().unwrap();
    let lockfile_path = temp.path().join("dmz.lock");
    let store_path = temp.path().join("store");

    let pkg = LockedPackage {
        name: "requests".to_string(),
        version: "2.31.0".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: "dummy".to_string(),
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    };
    DmzLockfile::new(vec![pkg]).save(&lockfile_path).unwrap();

    let count =
        DependencyFetcher::fetch_dependencies_with_bases(&lockfile_path, &store_path, &bases);
    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.join();

    assert!(count.is_ok(), "Failed to fetch: {:?}", count.err());
    assert_eq!(count.unwrap(), 5);

    let cn_hyphen = store_path.join("charset-normalizer");
    let cn_underscore = store_path.join("charset_normalizer");
    assert!(
        cn_hyphen.exists() || cn_underscore.exists(),
        "Expected charset-normalizer or charset_normalizer in store"
    );
}

#[test]
fn test_fetch_dependencies_pypi_legacy_dirty_lockfile_healed() {
    let (bases, _base_url, shutdown, handle) =
        spawn_mock_registry_server(|path, base| match path {
            "/pypi/charset_normalizer/2/json" | "/pypi/charset_normalizer/json" => {
                pypi_json_response(
                    base,
                    "charset-normalizer",
                    "2.0.0",
                    &[],
                    &["charset_normalizer-2.0.0-py3-none-any.whl"],
                )
            }
            "/files/charset_normalizer-2.0.0-py3-none-any.whl" => (
                "200 OK",
                "application/octet-stream",
                b"PK\x03\x04healed-wheel".to_vec(),
            ),
            _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
        });

    let temp = tempdir().unwrap();
    let lockfile_path = temp.path().join("dmz.lock");
    let store_path = temp.path().join("store");

    // Simulate an existing lockfile containing a dirty PEP 508 package name
    let pkg = LockedPackage {
        name: "charset_normalizer<4,".to_string(),
        version: "2".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: "dummy".to_string(),
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    };
    DmzLockfile::new(vec![pkg]).save(&lockfile_path).unwrap();

    let count =
        DependencyFetcher::fetch_dependencies_with_bases(&lockfile_path, &store_path, &bases);
    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.join();

    assert!(count.is_ok(), "Failed to fetch: {:?}", count.err());
    assert_eq!(count.unwrap(), 1);

    let reloaded = DmzLockfile::load_and_verify(&lockfile_path).unwrap();
    assert!(
        !reloaded.packages.contains_key("charset_normalizer<4,@2"),
        "Dirty key should have been removed from lockfile"
    );
    let has_clean = reloaded
        .packages
        .values()
        .any(|p| p.name == "charset_normalizer");
    assert!(has_clean, "Lockfile should contain sanitized package name");

    let cn_hyphen = store_path.join("charset-normalizer");
    let cn_underscore = store_path.join("charset_normalizer");
    assert!(
        cn_hyphen.exists() || cn_underscore.exists(),
        "Expected cached artifact in store"
    );
}

#[test]
fn test_fetch_dependencies_pypi_cuda_bindings_platform_wheel_fallback() {
    let (bases, _base_url, shutdown, handle) =
        spawn_mock_registry_server(|path, base| match path {
            "/pypi/cuda-bindings/13.5.0/json" | "/pypi/cuda-bindings/json" => pypi_json_response(
                base,
                "cuda-bindings",
                "13.5.0",
                &[],
                &[
                    "cuda_bindings-13.5.0-cp311-cp311-win_amd64.whl",
                    "cuda_bindings-13.5.0-cp311-cp311-manylinux_2_28_x86_64.whl",
                    "cuda_bindings-13.5.0-cp311-cp311-manylinux_2_28_aarch64.whl",
                ],
            ),
            p if p.starts_with("/files/") => (
                "200 OK",
                "application/octet-stream",
                b"PK\x03\x04mock-cuda-wheel".to_vec(),
            ),
            _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
        });

    let temp = tempdir().unwrap();
    let lockfile_path = temp.path().join("dmz.lock");
    let store_path = temp.path().join("store");

    // cuda-bindings distributes ONLY platform wheels (no sdist, no macOS wheels)
    let pkg = LockedPackage {
        name: "cuda-bindings".to_string(),
        version: "13.5.0".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: "dummy".to_string(),
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    };
    DmzLockfile::new(vec![pkg]).save(&lockfile_path).unwrap();

    let count =
        DependencyFetcher::fetch_dependencies_with_bases(&lockfile_path, &store_path, &bases);
    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.join();

    assert!(
        count.is_ok(),
        "Failed to fetch cuda-bindings: {:?}",
        count.err()
    );
    assert_eq!(count.unwrap(), 1);

    let cuda_dir = store_path.join("cuda-bindings");
    assert!(
        cuda_dir.exists(),
        "Expected cuda-bindings directory in store"
    );

    let entries: Vec<_> = std::fs::read_dir(&cuda_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("archive"))
        .collect();
    assert!(
        !entries.is_empty(),
        "Expected cached .archive file for cuda-bindings"
    );
}

#[test]
fn test_select_pypi_artifact_scoring_priorities() {
    let urls = vec![
        serde_json::json!({
            "filename": "cuda_bindings-13.5.0-cp311-cp311-win_amd64.whl",
            "url": "https://pypi.org/win_amd64"
        }),
        serde_json::json!({
            "filename": "cuda_bindings-13.5.0-cp311-cp311-manylinux_2_28_x86_64.whl",
            "url": "https://pypi.org/manylinux_x86_64"
        }),
        serde_json::json!({
            "filename": "cuda_bindings-13.5.0-cp311-cp311-manylinux_2_28_aarch64.whl",
            "url": "https://pypi.org/manylinux_aarch64"
        }),
    ];

    let selected = DependencyFetcher::select_pypi_artifact(&urls, None);
    assert!(selected.is_some());
    let sel_url = selected.unwrap();
    if cfg!(target_os = "windows") {
        assert!(sel_url.contains("win_amd64"));
    } else {
        assert!(sel_url.contains("manylinux"));
    }

    let selected_mac_arm = DependencyFetcher::select_pypi_artifact(&urls, Some("darwin-arm64"));
    assert_eq!(
        selected_mac_arm.as_deref(),
        Some("https://pypi.org/manylinux_aarch64")
    );

    let selected_linux = DependencyFetcher::select_pypi_artifact(&urls, Some("linux-x86_64"));
    assert_eq!(
        selected_linux.as_deref(),
        Some("https://pypi.org/manylinux_x86_64")
    );

    let selected_win = DependencyFetcher::select_pypi_artifact(&urls, Some("windows-x86_64"));
    assert_eq!(selected_win.as_deref(), Some("https://pypi.org/win_amd64"));
}

#[test]
fn test_fetch_dependencies_cached_artifact_fast_path() {
    use sha2::{Digest, Sha256};

    let temp = tempdir().unwrap();
    let lockfile_path = temp.path().join("dmz.lock");
    let store_path = temp.path().join("store");

    let payload = b"dummy cached content";
    let expected_sha256 =
        crate::archive::exporter::hashing::format_sha256_hex(&Sha256::digest(payload));

    let pkg = LockedPackage {
        name: "cached-pkg".to_string(),
        version: "1.0.0".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: expected_sha256,
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    };
    let lockfile = DmzLockfile::new(vec![pkg.clone()]);
    lockfile.save(&lockfile_path).unwrap();

    let pkg_dir = store_path.join("cached-pkg");
    std::fs::create_dir_all(&pkg_dir).unwrap();
    let artifact_path = pkg_dir.join("cached-pkg-1.0.0.archive");
    std::fs::write(&artifact_path, payload).unwrap();
    let meta_path = pkg_dir.join("metadata.json");
    std::fs::write(&meta_path, serde_json::to_string_pretty(&pkg).unwrap()).unwrap();

    let count = DependencyFetcher::fetch_dependencies(&lockfile_path, &store_path);
    assert!(
        count.is_ok(),
        "Failed to reuse cached artifact: {:?}",
        count.err()
    );
    assert_eq!(count.unwrap(), 1);
}

#[test]
fn test_fetch_dependencies_hermetic_localhost_registries() {
    use sha2::{Digest, Sha256};

    static ENV_LOCK: Mutex<()> = Mutex::new(());
    let _guard = ENV_LOCK.lock().unwrap();

    struct EnvReset;
    impl Drop for EnvReset {
        fn drop(&mut self) {
            for var in &[
                "DMZ_REGISTRY_PYPI",
                "DMZ_REGISTRY_CRATES_API",
                "DMZ_REGISTRY_CRATES_DL",
                "DMZ_REGISTRY_NPM",
                "DMZ_REGISTRY_GOLANG",
            ] {
                std::env::remove_var(var);
            }
        }
    }
    let _reset = EnvReset;

    let wheel_bytes = b"PK\x03\x04mock-wheel-bytes".to_vec();
    let dep_wheel_bytes = b"PK\x03\x04dep-wheel-bytes".to_vec();
    let crate_bytes = b"\x1f\x8b\x08\x00mock-crate-bytes".to_vec();
    let npm_bytes = b"\x1f\x8b\x08\x00mock-npm-tgz-bytes".to_vec();
    let go_zip_bytes = b"PK\x03\x04mock-go-mod-zip".to_vec();

    let expected_hashes = vec![
        crate::archive::format_sha256_hex(&Sha256::digest(&wheel_bytes)),
        crate::archive::format_sha256_hex(&Sha256::digest(&dep_wheel_bytes)),
        crate::archive::format_sha256_hex(&Sha256::digest(&crate_bytes)),
        crate::archive::format_sha256_hex(&Sha256::digest(&npm_bytes)),
        crate::archive::format_sha256_hex(&Sha256::digest(&go_zip_bytes)),
    ];

    let (bases, base_url, shutdown, handle) = spawn_mock_registry_server(move |path, base_srv| {
        let wheel_bytes = b"PK\x03\x04mock-wheel-bytes";
        let dep_wheel_bytes = b"PK\x03\x04dep-wheel-bytes";
        let crate_bytes = b"\x1f\x8b\x08\x00mock-crate-bytes";
        let npm_bytes = b"\x1f\x8b\x08\x00mock-npm-tgz-bytes";
        let go_zip_bytes = b"PK\x03\x04mock-go-mod-zip";

        match path {
            "/pypi/alpha/1.0.0/json" | "/pypi/alpha/json" => {
                let json = serde_json::json!({
                    "info": {
                        "version": "1.0.0",
                        "requires_dist": ["beta==0.5.0"]
                    },
                    "urls": [{
                        "filename": "alpha-1.0.0-py3-none-any.whl",
                        "url": format!("{}/files/alpha-1.0.0.whl", base_srv)
                    }]
                });
                ("200 OK", "application/json", json.to_string().into_bytes())
            }
            "/pypi/beta/0.5.0/json" | "/pypi/beta/json" => {
                let json = serde_json::json!({
                    "info": {
                        "version": "0.5.0",
                        "requires_dist": []
                    },
                    "urls": [{
                        "filename": "beta-0.5.0-py3-none-any.whl",
                        "url": format!("{}/files/beta-0.5.0.whl", base_srv)
                    }]
                });
                ("200 OK", "application/json", json.to_string().into_bytes())
            }
            "/files/alpha-1.0.0.whl" => {
                ("200 OK", "application/octet-stream", wheel_bytes.to_vec())
            }
            "/files/beta-0.5.0.whl" => (
                "200 OK",
                "application/octet-stream",
                dep_wheel_bytes.to_vec(),
            ),
            "/crates/mini_crate/mini_crate-0.2.0.crate" => {
                ("200 OK", "application/octet-stream", crate_bytes.to_vec())
            }
            "/npm/tiny_pkg/-/tiny_pkg-1.2.3.tgz" => {
                ("200 OK", "application/octet-stream", npm_bytes.to_vec())
            }
            "/golang/example.com/mod/@v/v0.3.0.zip" => {
                ("200 OK", "application/octet-stream", go_zip_bytes.to_vec())
            }
            _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
        }
    });
    let _ = bases;

    std::env::set_var("DMZ_REGISTRY_PYPI", format!("{}/pypi", base_url));
    std::env::set_var(
        "DMZ_REGISTRY_CRATES_API",
        format!("{}/crates_api", base_url),
    );
    std::env::set_var("DMZ_REGISTRY_CRATES_DL", format!("{}/crates", base_url));
    std::env::set_var("DMZ_REGISTRY_NPM", format!("{}/npm", base_url));
    std::env::set_var("DMZ_REGISTRY_GOLANG", format!("{}/golang", base_url));

    let temp = tempdir().unwrap();
    let lockfile_path = temp.path().join("dmz.lock");
    let store_path = temp.path().join("store");

    let packages = vec![
        LockedPackage {
            name: "alpha".to_string(),
            version: "1.0.0".to_string(),
            ecosystem: Ecosystem::PyPI,
            sha256: "pending".to_string(),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        },
        LockedPackage {
            name: "mini_crate".to_string(),
            version: "0.2.0".to_string(),
            ecosystem: Ecosystem::Cargo,
            sha256: "pending".to_string(),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        },
        LockedPackage {
            name: "tiny_pkg".to_string(),
            version: "1.2.3".to_string(),
            ecosystem: Ecosystem::Npm,
            sha256: "pending".to_string(),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        },
        LockedPackage {
            name: "example.com/mod".to_string(),
            version: "v0.3.0".to_string(),
            ecosystem: Ecosystem::Go,
            sha256: "pending".to_string(),
            source_url: None,
            dependencies: Vec::new(),
            member_id: None,
        },
    ];

    DmzLockfile::new(packages).save(&lockfile_path).unwrap();
    let count = DependencyFetcher::fetch_dependencies(&lockfile_path, &store_path);

    shutdown.store(true, Ordering::Relaxed);
    let _ = handle.join();

    assert!(count.is_ok(), "Offline fetch failed: {:?}", count.err());
    assert_eq!(count.unwrap(), 5);

    let reloaded = DmzLockfile::load_and_verify(&lockfile_path).unwrap();
    assert_eq!(reloaded.packages.len(), 5);
    for h in &expected_hashes {
        assert!(
            reloaded.packages.values().any(|p| &p.sha256 == h),
            "Missing expected real SHA-256 {} in lockfile",
            h
        );
    }
}

#[test]
fn test_fetch_dependencies_preserves_integrity_with_local_paths_and_sanitization() {
    let temp = tempdir().unwrap();
    let store_path = temp.path().join("store");

    let local_lock_path = temp.path().join("local.lock");
    let local_pkg = LockedPackage {
        name: "standalone-app".to_string(),
        version: "path:Cargo.toml".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: DmzLockfile::hash_bytes(b"standalone-app"),
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    };
    DmzLockfile::new(vec![local_pkg])
        .save(&local_lock_path)
        .unwrap();

    let count = DependencyFetcher::fetch_dependencies(&local_lock_path, &store_path).unwrap();
    assert_eq!(count, 0);
    let reloaded_local = DmzLockfile::load_and_verify(&local_lock_path).unwrap();
    assert!(reloaded_local
        .packages
        .contains_key("standalone-app:path:Cargo.toml"));

    let dirty_lock_path = temp.path().join("dirty_local.lock");
    let dirty_pkg = LockedPackage {
        name: "dirty_pkg>=1.0".to_string(),
        version: "path:local_pkg".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: DmzLockfile::hash_bytes(b"dirty_pkg"),
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    };
    DmzLockfile::new(vec![dirty_pkg])
        .save(&dirty_lock_path)
        .unwrap();

    let dirty_count = DependencyFetcher::fetch_dependencies(&dirty_lock_path, &store_path).unwrap();
    assert_eq!(dirty_count, 0);
    let reloaded_dirty = DmzLockfile::load_and_verify(&dirty_lock_path).unwrap();
    assert!(reloaded_dirty
        .packages
        .contains_key("dirty_pkg:path:local_pkg"));
    assert!(!reloaded_dirty
        .packages
        .contains_key("dirty_pkg>=1.0:path:local_pkg"));
}

#[test]
fn test_fetch_dependencies_npm_transitive_semver_and_cache_preserves_member_id() {
    let (_bases, base_url, stop, server_handle) = spawn_mock_registry_server(|path, base| {
        let vite_tgz = b"mock-vite-tarball-bytes".to_vec();
        let esbuild_tgz = b"mock-esbuild-tarball-bytes".to_vec();
        match path {
            "/vite" => {
                let json = format!(
                    r#"{{
                        "dist-tags": {{"latest": "6.0.11"}},
                        "versions": {{
                            "5.4.0": {{
                                "dist": {{"tarball": "{base}/vite/-/vite-5.4.0.tgz"}}
                            }},
                            "6.0.11": {{
                                "dependencies": {{"esbuild": "^0.24.0"}},
                                "optionalDependencies": {{"fsevents-win32-only-nonexistent-win32-x64": "1.0.0"}},
                                "dist": {{"tarball": "{base}/vite/-/vite-6.0.11.tgz"}}
                            }},
                            "7.0.0-alpha.1": {{
                                "dist": {{"tarball": "{base}/vite/-/vite-7.0.0-alpha.1.tgz"}}
                            }}
                        }}
                    }}"#
                );
                ("200 OK", "application/json", json.into_bytes())
            }
            "/esbuild" => {
                let json = format!(
                    r#"{{
                        "dist-tags": {{"latest": "0.25.0"}},
                        "versions": {{
                            "0.24.2": {{
                                "dist": {{"tarball": "{base}/esbuild/-/esbuild-0.24.2.tgz"}}
                            }},
                            "0.25.0": {{
                                "dist": {{"tarball": "{base}/esbuild/-/esbuild-0.25.0.tgz"}}
                            }}
                        }}
                    }}"#
                );
                ("200 OK", "application/json", json.into_bytes())
            }
            "/vite/-/vite-6.0.11.tgz" => ("200 OK", "application/octet-stream", vite_tgz),
            "/esbuild/-/esbuild-0.24.2.tgz" => ("200 OK", "application/octet-stream", esbuild_tgz),
            _ => ("404 Not Found", "text/plain", b"not found".to_vec()),
        }
    });

    let temp = tempdir().unwrap();
    let store_path = temp.path().join("store");

    let lock_a = temp.path().join("dmz_a.lock");
    let pkg_a = LockedPackage {
        name: "vite".to_string(),
        version: "^6.0.0".to_string(),
        ecosystem: Ecosystem::Npm,
        sha256: DmzLockfile::hash_bytes(b"vite:^6.0.0:Npm"),
        source_url: None,
        dependencies: Vec::new(),
        member_id: Some("member-a".to_string()),
    };
    DmzLockfile::new(vec![pkg_a]).save(&lock_a).unwrap();

    let count_a =
        DependencyFetcher::fetch_dependencies_with_npm_base(&lock_a, &store_path, &base_url)
            .expect("fetch run A");
    assert_eq!(count_a, 2, "Should fetch vite and transitive esbuild");

    let reloaded_a = DmzLockfile::load_and_verify(&lock_a).unwrap();
    let vite_a = reloaded_a
        .packages
        .values()
        .find(|p| p.name == "vite")
        .expect("vite in lockfile A");
    assert_eq!(vite_a.version, "6.0.11");
    assert_eq!(vite_a.member_id.as_deref(), Some("member-a"));

    let esbuild_a = reloaded_a
        .packages
        .values()
        .find(|p| p.name == "esbuild")
        .expect("transitive esbuild in lockfile A");
    assert_eq!(esbuild_a.version, "0.24.2");
    assert_eq!(esbuild_a.member_id.as_deref(), Some("member-a"));

    let lock_b = temp.path().join("dmz_b.lock");
    let pkg_b = LockedPackage {
        name: "vite".to_string(),
        version: "6.0.11".to_string(),
        ecosystem: Ecosystem::Npm,
        sha256: vite_a.sha256.clone(),
        source_url: None,
        dependencies: Vec::new(),
        member_id: Some("member-b".to_string()),
    };
    DmzLockfile::new(vec![pkg_b]).save(&lock_b).unwrap();

    let count_b =
        DependencyFetcher::fetch_dependencies_with_npm_base(&lock_b, &store_path, &base_url)
            .expect("fetch run B");
    assert_eq!(count_b, 2, "Cache hit on vite should still expand esbuild");

    let reloaded_b = DmzLockfile::load_and_verify(&lock_b).unwrap();
    let vite_b = reloaded_b
        .packages
        .values()
        .find(|p| p.name == "vite")
        .expect("vite in lockfile B");
    assert_eq!(vite_b.member_id.as_deref(), Some("member-b"));
    let esbuild_b = reloaded_b
        .packages
        .values()
        .find(|p| p.name == "esbuild")
        .expect("esbuild in lockfile B");
    assert_eq!(esbuild_b.member_id.as_deref(), Some("member-b"));

    stop.store(true, Ordering::Relaxed);
    let _ = server_handle.join();
}
