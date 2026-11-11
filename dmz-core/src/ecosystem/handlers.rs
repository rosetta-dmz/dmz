use super::{CommandSpec, EcosystemError, EcosystemHandler};
use std::path::{Path, PathBuf};

pub struct RustEcosystem;
impl EcosystemHandler for RustEcosystem {
    fn name(&self) -> &str {
        "rust"
    }

    fn detect(&self, root: &Path) -> bool {
        root.join("Cargo.toml").exists() || root.join("src-tauri/Cargo.toml").exists()
    }

    fn cache_paths(&self, root: &Path) -> Vec<PathBuf> {
        vec![
            root.join(".cargo"),
            root.join("target"),
            root.join("src-tauri/.cargo"),
            root.join("src-tauri/target"),
        ]
    }

    fn resolution_command(&self) -> Option<CommandSpec> {
        Some(CommandSpec {
            program: "cargo".into(),
            args: vec!["generate-lockfile".into(), "--offline".into()],
        })
    }

    fn resolution_command_for_workspace(&self, root: &Path) -> Option<CommandSpec> {
        let args =
            if !root.join("Cargo.toml").exists() && root.join("src-tauri/Cargo.toml").exists() {
                vec![
                    "generate-lockfile".into(),
                    "--offline".into(),
                    "--manifest-path".into(),
                    "src-tauri/Cargo.toml".into(),
                ]
            } else {
                vec!["generate-lockfile".into(), "--offline".into()]
            };
        Some(
            CommandSpec {
                program: "cargo".into(),
                args,
            }
            .resolve_in_workspace(root),
        )
    }

    fn verification_command(&self, root: &Path) -> Option<CommandSpec> {
        let args =
            if !root.join("Cargo.toml").exists() && root.join("src-tauri/Cargo.toml").exists() {
                vec![
                    "check".into(),
                    "--offline".into(),
                    "--manifest-path".into(),
                    "src-tauri/Cargo.toml".into(),
                ]
            } else {
                vec!["check".into(), "--offline".into()]
            };
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
                context: format!("Rust vendor materialization failed for {:?}", workspace_root),
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
        root.join("pnpm-lock.yaml").exists()
            || root.join("pnpm-workspace.yaml").exists()
            || root.join("node_modules/.pnpm").is_dir()
            || (root.join("package.json").exists() && {
                if let Ok(c) = std::fs::read_to_string(root.join("package.json")) {
                    c.contains("\"pnpm\"")
                } else {
                    false
                }
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
        Some(
            CommandSpec {
                program: "pnpm".into(),
                args: vec!["ls".into(), "--offline".into()],
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
        let is_yarn_project = root.join("yarn.lock").exists()
            || root.join(".yarnrc.yml").exists()
            || root.join(".yarnrc").exists()
            || (root.join("package.json").exists() && {
                if let Ok(c) = std::fs::read_to_string(root.join("package.json")) {
                    c.contains("\"yarn\"")
                } else {
                    false
                }
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
        Some(
            CommandSpec {
                program: "yarn".into(),
                args: vec!["list".into(), "--offline".into(), "--depth=0".into()],
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
        root.join("package.json").exists()
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
        Some(
            CommandSpec {
                program: "npm".into(),
                args: vec!["ls".into(), "--offline".into()],
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
            root.join(".venv/Scripts/pip.exe")
        } else {
            root.join(".venv/bin/pip")
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
            root.join(".venv/Scripts/python.exe")
        } else {
            root.join(".venv/bin/python")
        };
        if venv_py.is_file() {
            return Some(CommandSpec {
                program: venv_py.display().to_string(),
                args: vec!["-m".into(), "compileall".into(), "-q".into(), ".".into()],
            });
        }
        let py_prog = if cfg!(windows) { "python" } else { "python3" };
        Some(
            CommandSpec {
                program: py_prog.into(),
                args: vec!["-m".into(), "compileall".into(), "-q".into(), ".".into()],
            }
            .resolve_in_workspace(root),
        )
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

        let cmd_ambient = py.verification_command(ws).unwrap();
        #[cfg(not(windows))]
        assert_eq!(cmd_ambient.program, "python3");
        #[cfg(windows)]
        assert_eq!(cmd_ambient.program, "python");

        // When .venv exists, verification prefers the virtual environment binary
        let venv_bin = if cfg!(windows) {
            ws.join(".venv/Scripts")
        } else {
            ws.join(".venv/bin")
        };
        std::fs::create_dir_all(&venv_bin).unwrap();
        let venv_py = if cfg!(windows) {
            venv_bin.join("python.exe")
        } else {
            venv_bin.join("python")
        };
        std::fs::write(&venv_py, "").unwrap();

        let cmd_venv = py.verification_command(ws).unwrap();
        assert_eq!(cmd_venv.program, venv_py.display().to_string());
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
