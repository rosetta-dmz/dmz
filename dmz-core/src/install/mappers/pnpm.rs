#[cfg(windows)]
use crate::install::fast_clone::FastClone;
use crate::install::InstallError;
use crate::resolver::lockfile::{DmzLockfile, LockedPackage};
use crate::resolver::manifest::Ecosystem;
use std::fs;
use std::path::Path;
use tracing::warn;

/// pnpm Content-Addressable Virtual Store & Symlink Mapper
pub struct PnpmMapper;

impl PnpmMapper {
    fn link_dir_relative_or_clone(
        rel_target: &str,
        abs_target: &Path,
        link_path: &Path,
        pkg_label: &str,
    ) -> Result<(), InstallError> {
        #[cfg(not(windows))]
        let _ = abs_target;
        #[cfg(windows)]
        let _ = rel_target;

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            symlink(rel_target, link_path).map_err(|e| {
                InstallError::MapperError(format!(
                    "Failed to create pnpm symlink for {}: {}",
                    pkg_label, e
                ))
            })?;
        }

        #[cfg(windows)]
        {
            use std::os::windows::fs::symlink_dir;
            if symlink_dir(abs_target, link_path).is_err() {
                FastClone::clone_dir_recursive(abs_target, link_path).map_err(|e| {
                    InstallError::MapperError(format!(
                        "Failed to create pnpm directory link for {}: {}",
                        pkg_label, e
                    ))
                })?;
            }
        }
        Ok(())
    }

    pub fn map_package(
        workspace_path: &Path,
        pkg: &LockedPackage,
        archive_path: &Path,
    ) -> Result<(), InstallError> {
        let sanitized_name = pkg.name.replace('/', "+");
        let virtual_store_dir = workspace_path
            .join("node_modules")
            .join(".pnpm")
            .join(format!("{}@{}", sanitized_name, pkg.version))
            .join("node_modules")
            .join(&pkg.name);

        fs::create_dir_all(&virtual_store_dir).map_err(|e| {
            InstallError::IoError(format!("Failed to create pnpm virtual store dir: {}", e))
        })?;

        super::npm::NpmMapper::unpack_archive(archive_path, &virtual_store_dir, &pkg.name)?;
        super::npm::NpmMapper::link_package_binaries(
            workspace_path,
            &virtual_store_dir,
            &pkg.name,
        )?;

        let root_link_dir = workspace_path.join("node_modules").join(&pkg.name);
        if let Some(parent) = root_link_dir.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                InstallError::IoError(format!("Failed to create root node_modules dir: {}", e))
            })?;
        }

        // Preserve existing root symlink when another version of the same package is already linked
        // so multi-version transitive graphs never overwrite each other at `node_modules/<name>`.
        let version_marker = format!("{}@{}", sanitized_name, pkg.version);
        if let Ok(meta) = fs::symlink_metadata(&root_link_dir) {
            if meta.file_type().is_symlink() {
                if let Ok(existing_target) = fs::read_link(&root_link_dir) {
                    if root_link_dir.exists()
                        && !existing_target.to_string_lossy().contains(&version_marker)
                    {
                        Self::ensure_modules_yaml(workspace_path);
                        return Ok(());
                    }
                }
                fs::remove_file(&root_link_dir).map_err(|e| {
                    InstallError::IoError(format!(
                        "Failed to replace pnpm symlink {:?}: {}",
                        root_link_dir, e
                    ))
                })?;
            } else {
                fs::remove_dir_all(&root_link_dir).map_err(|e| {
                    InstallError::IoError(format!(
                        "Failed to replace root node_modules dir {:?}: {}",
                        root_link_dir, e
                    ))
                })?;
            }
        }

        let rel_target = if pkg.name.contains('/') {
            format!(
                "../.pnpm/{}@{}/node_modules/{}",
                sanitized_name, pkg.version, pkg.name
            )
        } else {
            format!(
                ".pnpm/{}@{}/node_modules/{}",
                sanitized_name, pkg.version, pkg.name
            )
        };
        Self::link_dir_relative_or_clone(
            &rel_target,
            &virtual_store_dir,
            &root_link_dir,
            &pkg.name,
        )?;

        Self::ensure_modules_yaml(workspace_path);

        Ok(())
    }

    /// Creates sibling symlinks under `node_modules/.pnpm/<sanitized>@<ver>/node_modules/<dep>`
    /// so transitive and multi-version dependencies resolve according to pnpm strict isolation.
    pub fn wire_pnpm_dependency_links(
        workspace_path: &Path,
        lockfile: &DmzLockfile,
    ) -> Result<(), InstallError> {
        let pnpm_store = workspace_path.join("node_modules").join(".pnpm");
        if !pnpm_store.is_dir() {
            return Ok(());
        }

        let npm_pkgs: Vec<&LockedPackage> = lockfile
            .packages
            .values()
            .filter(|p| p.ecosystem == Ecosystem::Npm && !p.is_local_path())
            .collect();

        for pkg in &npm_pkgs {
            if pkg.dependencies.is_empty() {
                continue;
            }
            let sanitized_pkg = pkg.name.replace('/', "+");
            let pkg_nm_root = pnpm_store
                .join(format!("{}@{}", sanitized_pkg, pkg.version))
                .join("node_modules");
            if !pkg_nm_root.is_dir() {
                continue;
            }

            for dep_spec in &pkg.dependencies {
                let (dep_name, dep_ver_req) = Self::split_dep_spec(dep_spec);
                let clean_ver = dep_ver_req
                    .trim_start_matches('^')
                    .trim_start_matches('~')
                    .trim_start_matches('=')
                    .trim();

                let Some(target_pkg) = npm_pkgs
                    .iter()
                    .find(|cand| cand.name == dep_name && cand.version == clean_ver)
                    .or_else(|| {
                        npm_pkgs.iter().find(|cand| {
                            cand.name == dep_name
                                && !clean_ver.is_empty()
                                && cand.version.starts_with(clean_ver)
                        })
                    })
                    .or_else(|| npm_pkgs.iter().find(|cand| cand.name == dep_name))
                else {
                    continue;
                };

                let link_path = pkg_nm_root.join(&target_pkg.name);
                if fs::symlink_metadata(&link_path).is_ok() {
                    continue;
                }
                if let Some(parent) = link_path.parent() {
                    fs::create_dir_all(parent).map_err(|e| {
                        InstallError::IoError(format!(
                            "Failed to create scoped pnpm dep dir {:?}: {}",
                            parent, e
                        ))
                    })?;
                }

                let target_sanitized = target_pkg.name.replace('/', "+");
                let depth_prefix = if target_pkg.name.contains('/') {
                    "../../../"
                } else {
                    "../../"
                };
                let rel_target = format!(
                    "{}{}@{}/node_modules/{}",
                    depth_prefix, target_sanitized, target_pkg.version, target_pkg.name
                );
                let abs_target = pnpm_store
                    .join(format!("{}@{}", target_sanitized, target_pkg.version))
                    .join("node_modules")
                    .join(&target_pkg.name);

                if abs_target.is_dir() {
                    Self::link_dir_relative_or_clone(
                        &rel_target,
                        &abs_target,
                        &link_path,
                        &target_pkg.name,
                    )?;
                }
            }
        }

        Ok(())
    }

    fn split_dep_spec(spec: &str) -> (&str, &str) {
        if let Some((prefix, ver)) = spec.rsplit_once('@') {
            if !prefix.is_empty() {
                return (prefix, ver);
            }
        }
        (spec, "")
    }

    /// Writes node_modules/.modules.yaml so that pnpm CLI recognizes node_modules as installed
    /// and avoids the error: "Local package.json exists, but node_modules missing, did you mean to install?"
    pub fn ensure_modules_yaml(workspace_path: &Path) {
        let nm_dir = workspace_path.join("node_modules");
        if !nm_dir.is_dir() {
            if let Err(e) = fs::create_dir_all(&nm_dir) {
                warn!(dir = ?nm_dir, error = %e, "Failed to create node_modules for .modules.yaml");
                return;
            }
        }
        let modules_yaml_path = nm_dir.join(".modules.yaml");
        if !modules_yaml_path.exists() {
            let is_pnpm_store = nm_dir.join(".pnpm").is_dir();
            let node_linker = if is_pnpm_store { "isolated" } else { "hoisted" };
            let content = format!(
                r#"# Generated by DMZ offline installer
alreadyPinnedVersions: {{}}
hoistedDependencies: {{}}
included:
  dependencies: true
  devDependencies: true
  optionalDependencies: true
injectedDeps: {{}}
layoutVersion: 5
nodeLinker: {node_linker}
packageManager: pnpm
pendingBuilds: []
prunedAt: '2026-01-01T00:00:00.000Z'
publicHoistPattern: []
registries:
  default: https://registry.npmjs.org/
skipped: []
storeDir: .pnpm-store
virtualStoreDir: .pnpm
virtualStoreDirMaxLength: 120
"#
            );
            if let Err(e) = fs::write(&modules_yaml_path, content) {
                warn!(path = ?modules_yaml_path, error = %e, "Failed to write .modules.yaml");
            }
        }
    }
}
