use super::{CommandSpec, EcosystemError, EcosystemHandler};
use std::path::{Path, PathBuf};

fn first_nested_cargo_manifest_rel(root: &Path) -> Option<String> {
    if root.join("Cargo.toml").exists() {
        return None;
    }
    crate::workspace::discover_tauri_cargo_dirs(root)
        .into_iter()
        .find_map(|dir| {
            let rel = dir.strip_prefix(root).ok()?;
            Some(rel.join("Cargo.toml").to_string_lossy().into_owned())
        })
}

fn first_nested_node_dir_rel(root: &Path, marker: Option<&str>) -> Option<String> {
    if root.join("package.json").exists() {
        return None;
    }
    let dirs = crate::workspace::discover_node_project_roots(root);
    let chosen = marker
        .and_then(|file| dirs.iter().find(|d| d.join(file).exists()))
        .or_else(|| dirs.first())?;
    let rel = chosen.strip_prefix(root).ok()?;
    if rel.as_os_str().is_empty() {
        None
    } else {
        Some(rel.to_string_lossy().into_owned())
    }
}

pub struct RustEcosystem;
impl EcosystemHandler for RustEcosystem {
    fn name(&self) -> &str {
        "rust"
    }

    fn detect(&self, root: &Path) -> bool {
        !crate::workspace::discover_tauri_cargo_dirs(root).is_empty()
    }

    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        let mut paths = vec![root.join(".cargo"), root.join("target")];
        for dir in crate::workspace::discover_tauri_cargo_dirs(root) {
            if dir != root {
                paths.push(dir.join(".cargo"));
                paths.push(dir.join("target"));
            }
        }
        paths
    }

    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "cargo".into(),
            args: vec!["generate-lockfile".into(), "--offline".into()],
        })
    }

    fn resolution_command_for_workspace(&self, root: &Path) -> Option<CommandSpec> {
        let mut args = vec!["generate-lockfile".into(), "--offline".into()];
        if let Some(rel_manifest) = first_nested_cargo_manifest_rel(root) {
            args.push("--manifest-path".into());
            args.push(rel_manifest);
        }
        Some(
            CommandSpec {
                program: "cargo".into(),
                args,
            }
            .resolve_in_workspace(root),
        )
    }

    fn verification_command(&self, root: &Path) -> Option<CommandSpec> {
        let mut args = vec!["check".into(), "--offline".into()];
        if let Some(rel_manifest) = first_nested_cargo_manifest_rel(root) {
            args.push("--manifest-path".into());
            args.push(rel_manifest);
        }
        Some(
            CommandSpec {
                program: "cargo".into(),
                args,
            }
            .resolve_in_workspace(root),
        )
    }

    fn materialize(
        &self,
        workspace_root: &Path,
        offline_store: &Path,
        lockfile_path: &Path,
    ) -> Result<(), EcosystemError> {
        let materializer = crate::install::rust_vendor::RustVendorMaterializer::new(
            workspace_root.to_path_buf(),
            offline_store.to_path_buf(),
        );
        materializer
            .materialize(lockfile_path)
            .map_err(|e| EcosystemError::Io {
                context: format!(
                    "Rust vendor materialization failed for {:?}",
                    workspace_root
                ),
                source: e,
            })?;
        Ok(())
    }
}

pub struct PnpmEcosystem;
impl EcosystemHandler for PnpmEcosystem {
    fn name(&self) -> &str {
        "pnpm"
    }

    fn detect(&self, root: &Path) -> bool {
        crate::workspace::discover_node_project_roots(root)
            .iter()
            .any(|node_root| {
                node_root.join("pnpm-lock.yaml").exists()
                    || node_root.join("pnpm-workspace.yaml").exists()
                    || node_root.join("node_modules/.pnpm").is_dir()
                    || (node_root.join("package.json").exists() && {
                        if let Ok(c) = std::fs::read_to_string(node_root.join("package.json")) {
                            c.contains("\"pnpm\"")
                        } else {
                            false
                        }
                    })
            })
    }

    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("node_modules"), root.join(".pnpm-store")]
    }

    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "pnpm".into(),
            args: vec![
                "install".into(),
                "--frozen-lockfile".into(),
                "--offline".into(),
            ],
        })
    }

    fn verification_command(&self, root: &Path) -> Option<CommandSpec> {
        let mut args = Vec::new();
        if let Some(rel_dir) = first_nested_node_dir_rel(root, Some("pnpm-lock.yaml")) {
            args.push("-C".into());
            args.push(rel_dir);
        }
        args.push("ls".into());
        args.push("--offline".into());
        Some(
            CommandSpec {
                program: "pnpm".into(),
                args,
            }
            .resolve_in_workspace(root),
        )
    }
}

pub struct YarnEcosystem;
impl EcosystemHandler for YarnEcosystem {
    fn name(&self) -> &str {
        "yarn"
    }

    fn detect(&self, root: &Path) -> bool {
        let is_yarn_project = crate::workspace::discover_node_project_roots(root)
            .iter()
            .any(|node_root| {
                node_root.join("yarn.lock").exists()
                    || node_root.join(".yarnrc.yml").exists()
                    || node_root.join(".yarnrc").exists()
                    || (node_root.join("package.json").exists() && {
                        if let Ok(c) = std::fs::read_to_string(node_root.join("package.json")) {
                            c.contains("\"yarn\"")
                        } else {
                            false
                        }
                    })
            });

        is_yarn_project && !PnpmEcosystem.detect(root)
    }

    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("node_modules"), root.join(".yarn/cache")]
    }

    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "yarn".into(),
            args: vec![
                "install".into(),
                "--frozen-lockfile".into(),
                "--offline".into(),
            ],
        })
    }

    fn verification_command(&self, root: &Path) -> Option<CommandSpec> {
        let mut args = Vec::new();
        if let Some(rel_dir) = first_nested_node_dir_rel(root, Some("yarn.lock")) {
            args.push("--cwd".into());
            args.push(rel_dir);
        }
        args.push("list".into());
        args.push("--offline".into());
        args.push("--depth=0".into());
        Some(
            CommandSpec {
                program: "yarn".into(),
                args,
            }
            .resolve_in_workspace(root),
        )
    }
}

pub struct NpmEcosystem;
impl EcosystemHandler for NpmEcosystem {
    fn name(&self) -> &str {
        "npm"
    }

    fn detect(&self, root: &Path) -> bool {
        crate::workspace::discover_node_project_roots(root)
            .iter()
            .any(|node_root| node_root.join("package.json").exists())
            && !PnpmEcosystem.detect(root)
            && !YarnEcosystem.detect(root)
    }

    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("node_modules"), root.join(".npm")]
    }

    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "npm".into(),
            args: vec!["ci".into(), "--prefer-offline".into()],
        })
    }

    fn verification_command(&self, root: &Path) -> Option<CommandSpec> {
        let mut args = vec!["ls".into(), "--offline".into()];
        if let Some(rel_dir) = first_nested_node_dir_rel(root, None) {
            args.push("--prefix".into());
            args.push(rel_dir);
        }
        Some(
            CommandSpec {
                program: "npm".into(),
                args,
            }
            .resolve_in_workspace(root),
        )
    }
}

pub struct GoEcosystem;
impl EcosystemHandler for GoEcosystem {
    fn name(&self) -> &str {
        "go"
    }

    fn detect(&self, root: &Path) -> bool {
        root.join("go.mod").exists()
    }

    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("vendor")]
    }

    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "go".into(),
            args: vec!["mod".into(), "download".into()],
        })
    }

    fn verification_command(&self, root: &Path) -> Option<CommandSpec> {
        let mod_flag = if root.join("vendor").is_dir() {
            "-mod=vendor"
        } else {
            "-mod=readonly"
        };
        Some(
            CommandSpec {
                program: "go".into(),
                args: vec!["build".into(), mod_flag.into(), "./...".into()],
            }
            .resolve_in_workspace(root),
        )
    }
}

pub struct PythonEcosystem;
impl EcosystemHandler for PythonEcosystem {
    fn name(&self) -> &str {
        "python"
    }

    fn detect(&self, root: &Path) -> bool {
        root.join("pyproject.toml").exists() || root.join("requirements.txt").exists()
    }

    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join(".venv"), root.join("__pycache__")]
    }

    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "pip".into(),
            args: vec![
                "install".into(),
                "--no-deps".into(),
                "-r".into(),
                "requirements.txt".into(),
            ],
        })
    }

    fn resolution_command_for_workspace(&self, root: &Path) -> Option<CommandSpec> {
        let venv_pip = if cfg!(windows) {
            root.join(".venv").join("Scripts").join("pip.exe")
        } else {
            root.join(".venv").join("bin").join("pip")
        };

        let req_args = if root.join("requirements.txt").exists() {
            vec![
                "install".into(),
                "--no-deps".into(),
                "-r".into(),
                "requirements.txt".into(),
            ]
        } else {
            vec![
                "install".into(),
                "--no-deps".into(),
                "--no-build-isolation".into(),
                ".".into(),
            ]
        };

        if venv_pip.is_file() {
            return Some(CommandSpec {
                program: venv_pip.display().to_string(),
                args: req_args,
            });
        }

        Some(
            CommandSpec {
                program: "pip".into(),
                args: req_args,
            }
            .resolve_in_workspace(root),
        )
    }

    fn verification_command(&self, root: &Path) -> Option<CommandSpec> {
        let venv_py = if cfg!(windows) {
            root.join(".venv").join("Scripts").join("python.exe")
        } else {
            root.join(".venv").join("bin").join("python")
        };
        if venv_py.is_file() && !crate::toolchain::is_uninstalled_macos_clt_shim(&venv_py, root) {
            return Some(CommandSpec {
                program: venv_py.display().to_string(),
                args: vec!["-m".into(), "compileall".into(), "-q".into(), ".".into()],
            });
        }
        let py_prog = if cfg!(windows) { "python" } else { "python3" };
        let resolved_ws = CommandSpec {
            program: py_prog.into(),
            args: vec!["-m".into(), "compileall".into(), "-q".into(), ".".into()],
        }
        .resolve_in_workspace(root);

        if Path::new(&resolved_ws.program).is_absolute()
            && !crate::toolchain::is_uninstalled_macos_clt_shim(
                Path::new(&resolved_ws.program),
                root,
            )
        {
            return Some(resolved_ws);
        }

        // Only return a verified non-stub interpreter path (never bare `python`/`python3`,
        // which can still resolve to `/usr/bin/python3` Apple CLT GUI stub ahead of bundled/other entries).
        crate::install::mappers::PyPiMapper::resolve_non_stub_host_python(root).map(|resolved_py| {
            CommandSpec {
                program: resolved_py.display().to_string(),
                args: vec!["-m".into(), "compileall".into(), "-q".into(), ".".into()],
            }
        })
    }
}

pub struct PhpEcosystem;
impl EcosystemHandler for PhpEcosystem {
    fn name(&self) -> &str {
        "php"
    }

    fn detect(&self, root: &Path) -> bool {
        root.join("composer.json").exists()
    }

    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("vendor")]
    }

    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "composer".into(),
            args: vec!["install".into(), "--no-dev".into()],
        })
    }

    fn verification_command(&self, root: &Path) -> Option<CommandSpec> {
        Some(
            CommandSpec {
                program: "composer".into(),
                args: vec!["validate".into()],
            }
            .resolve_in_workspace(root),
        )
    }
}

pub struct RubyEcosystem;
impl EcosystemHandler for RubyEcosystem {
    fn name(&self) -> &str {
        "ruby"
    }

    fn detect(&self, root: &Path) -> bool {
        root.join("Gemfile").exists()
    }

    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![root.join("vendor/bundle"), root.join(".bundle")]
    }

    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "bundle".into(),
            args: vec![
                "install".into(),
                "--local".into(),
                "--path".into(),
                "vendor/bundle".into(),
            ],
        })
    }

    fn verification_command(&self, root: &Path) -> Option<CommandSpec> {
        Some(
            CommandSpec {
                program: "bundle".into(),
                args: vec!["check".into()],
            }
            .resolve_in_workspace(root),
        )
    }
}

pub struct MavenGradleEcosystem;
impl EcosystemHandler for MavenGradleEcosystem {
    fn name(&self) -> &str {
        "maven"
    }

    fn detect(&self, root: &Path) -> bool {
        root.join("pom.xml").exists()
            || root.join("build.gradle").exists()
            || root.join("build.gradle.kts").exists()
    }

    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![
            root.join(".m2"),
            root.join(".gradle"),
            root.join("target"),
            root.join("build"),
        ]
    }

    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "mvn".into(),
            args: vec!["dependency:go-offline".into()],
        })
    }

    fn verification_command(&self, root: &Path) -> Option<CommandSpec> {
        if root.join("pom.xml").exists() {
            Some(
                CommandSpec {
                    program: "mvn".into(),
                    args: vec!["compile".into(), "-o".into()],
                }
                .resolve_in_workspace(root),
            )
        } else if root.join("gradlew").is_file() {
            #[cfg(windows)]
            let gradlew = root.join("gradlew.bat");
            #[cfg(not(windows))]
            let gradlew = root.join("gradlew");

            let prog = if gradlew.is_file() {
                gradlew.display().to_string()
            } else {
                "gradle".to_string()
            };
            Some(
                CommandSpec {
                    program: prog,
                    args: vec!["build".into(), "--offline".into()],
                }
                .resolve_in_workspace(root),
            )
        } else {
            Some(
                CommandSpec {
                    program: "gradle".into(),
                    args: vec!["build".into(), "--offline".into()],
                }
                .resolve_in_workspace(root),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_rust_ecosystem_detection_and_tauri_fallback() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let rust = RustEcosystem;
        assert!(!rust.detect(ws));

        // Subdirectory Tauri detection
        std::fs::create_dir_all(ws.join("src-tauri")).unwrap();
        std::fs::write(ws.join("src-tauri/Cargo.toml"), "[package]").unwrap();
        assert!(rust.detect(ws));

        let tauri_cmd = rust.verification_command(ws).unwrap();
        assert_eq!(tauri_cmd.program, "cargo");
        assert!(tauri_cmd.args.contains(&"--manifest-path".to_string()));
        assert!(tauri_cmd.args.contains(&"src-tauri/Cargo.toml".to_string()));

        let tauri_res_cmd = rust.resolution_command_for_workspace(ws).unwrap();
        assert!(tauri_res_cmd.args.contains(&"--manifest-path".to_string()));

        // Root Cargo.toml takes precedence
        std::fs::write(ws.join("Cargo.toml"), "[package]").unwrap();
        let root_cmd = rust.verification_command(ws).unwrap();
        assert!(!root_cmd.args.contains(&"--manifest-path".to_string()));
    }

    #[test]
    fn test_node_ecosystems_mutual_exclusion() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let pnpm = PnpmEcosystem;
        let yarn = YarnEcosystem;
        let npm = NpmEcosystem;

        // Clean directory: none detected
        assert!(!pnpm.detect(ws));
        assert!(!yarn.detect(ws));
        assert!(!npm.detect(ws));

        // Plain package.json -> Npm detected, others false
        std::fs::write(ws.join("package.json"), "{}").unwrap();
        assert!(!pnpm.detect(ws));
        assert!(!yarn.detect(ws));
        assert!(npm.detect(ws));

        // Adding yarn.lock -> Yarn detected, npm false
        std::fs::write(ws.join("yarn.lock"), "").unwrap();
        assert!(!pnpm.detect(ws));
        assert!(yarn.detect(ws));
        assert!(!npm.detect(ws));

        // Adding pnpm-lock.yaml -> Pnpm detected, yarn & npm false
        std::fs::write(ws.join("pnpm-lock.yaml"), "").unwrap();
        assert!(pnpm.detect(ws));
        assert!(!yarn.detect(ws));
        assert!(!npm.detect(ws));
    }

    #[test]
    fn test_go_verification_command_vendor_toggle() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let go = GoEcosystem;

        std::fs::write(ws.join("go.mod"), "module test").unwrap();
        assert!(go.detect(ws));

        let cmd_readonly = go.verification_command(ws).unwrap();
        assert!(cmd_readonly.args.contains(&"-mod=readonly".to_string()));

        std::fs::create_dir_all(ws.join("vendor")).unwrap();
        let cmd_vendor = go.verification_command(ws).unwrap();
        assert!(cmd_vendor.args.contains(&"-mod=vendor".to_string()));
    }

    #[test]
    fn test_python_venv_resolution_priority() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let py = PythonEcosystem;

        std::fs::write(ws.join("requirements.txt"), "").unwrap();
        assert!(py.detect(ws));

        if let Some(expected_py) =
            crate::install::mappers::PyPiMapper::resolve_non_stub_host_python(ws)
        {
            let cmd_ambient = py.verification_command(ws).unwrap();
            assert_eq!(Path::new(&cmd_ambient.program), expected_py.as_path());
        } else {
            assert!(py.verification_command(ws).is_none());
        }

        // When .venv exists, verification prefers the virtual environment binary
        let venv_bin = if cfg!(windows) {
            ws.join(".venv").join("Scripts")
        } else {
            ws.join(".venv").join("bin")
        };
        std::fs::create_dir_all(&venv_bin).unwrap();
        let venv_py = if cfg!(windows) {
            venv_bin.join("python.exe")
        } else {
            venv_bin.join("python")
        };
        std::fs::write(&venv_py, "").unwrap();

        let cmd_venv = py.verification_command(ws).unwrap();
        assert_eq!(Path::new(&cmd_venv.program), venv_py.as_path());
    }

    #[test]
    fn test_maven_gradle_detection_and_verification() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let mg = MavenGradleEcosystem;

        assert!(!mg.detect(ws));

        std::fs::write(ws.join("pom.xml"), "<project></project>").unwrap();
        assert!(mg.detect(ws));

        let cmd_mvn = mg.verification_command(ws).unwrap();
        assert_eq!(cmd_mvn.program, "mvn");
        assert!(cmd_mvn.args.contains(&"-o".to_string()));
    }

    #[test]
    fn test_command_spec_workspace_toolchain_resolution() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let tc_bin_dir = ws.join(".dmz/toolchains/custom/bin");
        std::fs::create_dir_all(&tc_bin_dir).unwrap();
        let custom_bin = tc_bin_dir.join("pnpm");
        std::fs::write(&custom_bin, "").unwrap();

        let cmd = CommandSpec::new("pnpm", vec!["ls".into()]).resolve_in_workspace(ws);
        assert_eq!(
            cmd.program.replace('\\', "/"),
            custom_bin.to_string_lossy().replace('\\', "/")
        );
    }
}
