use super::types::ImportError;
use std::fs;
use std::io::Read;
use std::path::Path;

pub fn is_safe_relative_path(path: &Path) -> bool {
    !path.is_absolute()
        && !path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
}

pub fn is_contained_path(base_dir: &Path, rel_link: &Path) -> bool {
    let mut depth: isize = 0;
    for comp in base_dir.components() {
        match comp {
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::ParentDir => depth -= 1,
            _ => {}
        }
    }
    for comp in rel_link.components() {
        match comp {
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => return false,
            std::path::Component::CurDir => {}
        }
    }
    depth >= 0
}

pub fn unpack_tar_entry_contained<R: Read>(
    entry: &mut tar::Entry<R>,
    target_root: &Path,
    rel_path: &Path,
) -> Result<(), ImportError> {
    if !is_safe_relative_path(rel_path) {
        return Err(ImportError::ExtractionError(format!(
            "Path traversal attempt detected in archive entry: {:?}",
            rel_path
        )));
    }

    let dest_path = target_root.join(rel_path);

    // Validate link target if entry is a symlink or hard link
    let entry_type = entry.header().entry_type();
    if entry_type.is_symlink() || entry_type.is_hard_link() {
        let link_target = entry
            .link_name()
            .map_err(|e| ImportError::ExtractionError(format!("Invalid link header: {}", e)))?
            .ok_or_else(|| {
                ImportError::ExtractionError("Missing link target for link entry".to_string())
            })?;

        let rel_parent = rel_path.parent().unwrap_or(Path::new(""));
        if !is_contained_path(rel_parent, &link_target) {
            return Err(ImportError::ExtractionError(format!(
                "Path traversal attempt detected in link target: {:?} -> {:?}",
                rel_path, link_target
            )));
        }

        if entry_type.is_hard_link() {
            let hard_source = target_root.join(rel_parent).join(&link_target);
            let canon_root =
                fs::canonicalize(target_root).unwrap_or_else(|_| target_root.to_path_buf());
            let canon_source =
                fs::canonicalize(&hard_source).unwrap_or_else(|_| hard_source.clone());
            if !canon_source.starts_with(&canon_root) {
                return Err(ImportError::ExtractionError(format!(
                    "Hardlink source escapes extraction root: {:?}",
                    hard_source
                )));
            }
        }
    }

    // Verify intermediate directories between target_root and dest_path.parent()
    let mut current = target_root.to_path_buf();
    if let Some(parent) = rel_path.parent() {
        for component in parent.components() {
            if let std::path::Component::Normal(comp) = component {
                current.push(comp);
                if current.exists() {
                    let meta = fs::symlink_metadata(&current).map_err(|e| {
                        ImportError::IoError(format!(
                            "Failed to inspect directory component {:?}: {}",
                            current, e
                        ))
                    })?;
                    if meta.file_type().is_symlink() {
                        return Err(ImportError::ExtractionError(format!(
                            "Symlink traversal detected through intermediate directory {:?}",
                            current
                        )));
                    }
                } else {
                    fs::create_dir_all(&current).map_err(|e| {
                        ImportError::IoError(format!("Failed to create directory {:?}: {}", current, e))
                    })?;
                }
            }
        }
    }

    // If dest_path is an existing symlink or file, remove it before extraction
    if dest_path.exists() || fs::symlink_metadata(&dest_path).is_ok() {
        if let Ok(m) = fs::symlink_metadata(&dest_path) {
            if m.file_type().is_symlink() || m.is_file() {
                let _ = fs::remove_file(&dest_path);
            }
        }
    }

    // Unpack according to entry type
    if entry_type.is_dir() {
        fs::create_dir_all(&dest_path).map_err(|e| {
            ImportError::IoError(format!("Failed to create directory {:?}: {}", dest_path, e))
        })?;
    } else if entry_type.is_symlink() {
        let link_target = entry
            .link_name()
            .map_err(|e| ImportError::ExtractionError(e.to_string()))?
            .ok_or_else(|| {
                ImportError::ExtractionError(format!(
                    "Malformed archive entry {:?}: missing symlink target",
                    dest_path
                ))
            })?;

        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&link_target, &dest_path).map_err(|e| {
                ImportError::ExtractionError(format!(
                    "Failed to create symlink {:?} -> {:?}: {}",
                    dest_path, link_target, e
                ))
            })?;
        }
        #[cfg(windows)]
        {
            if let Err(e) = std::os::windows::fs::symlink_file(&link_target, &dest_path) {
                tracing::warn!(
                    link_target = ?link_target,
                    dest_path = ?dest_path,
                    error = %e,
                    "Failed to create Windows symlink; attempting fallback copy"
                );
                let source_path = target_root
                    .join(rel_path.parent().unwrap_or(Path::new("")))
                    .join(&link_target);
                if source_path.exists() {
                    let _ = std::fs::copy(&source_path, &dest_path);
                }
            }
        }
    } else if entry_type.is_hard_link() {
        let link_target = entry
            .link_name()
            .map_err(|e| ImportError::ExtractionError(e.to_string()))?
            .ok_or_else(|| {
                ImportError::ExtractionError(format!(
                    "Malformed archive entry {:?}: missing hard link target",
                    dest_path
                ))
            })?;
        let rel_parent = rel_path.parent().unwrap_or(Path::new(""));
        let source_path = target_root.join(rel_parent).join(&link_target);
        fs::hard_link(&source_path, &dest_path).map_err(|e| {
            ImportError::ExtractionError(format!(
                "Failed to create hard link {:?} -> {:?}: {}",
                dest_path, source_path, e
            ))
        })?;
    } else {
        // Regular file
        if let Some(parent) = dest_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    ImportError::IoError(format!("Failed to create path {:?}: {}", parent, e))
                })?;
            }
        }
        let mut outfile = fs::File::create(&dest_path).map_err(|e| {
            ImportError::IoError(format!("Failed to create file {:?}: {}", dest_path, e))
        })?;
        std::io::copy(entry, &mut outfile).map_err(|e| {
            ImportError::IoError(format!("Failed to extract file {:?}: {}", dest_path, e))
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = entry.header().mode().unwrap_or(0o644) & 0o777;
            let _ = fs::set_permissions(&dest_path, fs::Permissions::from_mode(mode));
        }
    }

    Ok(())
}

