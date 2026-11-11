use super::types::ImportError;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::info;

pub fn create_backup_snapshot(workspace: &Path) -> Result<Option<PathBuf>, ImportError> {
    if !workspace.exists() {
        return Ok(None);
    }
    let entries = match fs::read_dir(workspace) {
        Ok(e) => e,
        Err(_) => return Ok(None),
    };

    let mut has_files = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str != ".dmz"
            && name_str != "target"
            && name_str != "dist"
            && name_str != "node_modules"
        {
            has_files = true;
            break;
        }
    }

    if !has_files {
        return Ok(None);
    }

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let backup_dir = workspace
        .join(".dmz/backups")
        .join(format!("snapshot-{}", timestamp));
    fs::create_dir_all(&backup_dir).map_err(|e| {
        ImportError::IoError(format!(
            "Failed to create backup snapshot dir {:?}: {}",
            backup_dir, e
        ))
    })?;

    copy_dir_contents(workspace, &backup_dir, true)?;
    info!(backup = ?backup_dir, "Created pre-import workspace backup snapshot");
    Ok(Some(backup_dir))
}

pub fn restore_backup_snapshot(workspace: &Path, backup_dir: &Path) -> Result<(), ImportError> {
    info!(
        backup = ?backup_dir,
        workspace = ?workspace,
        "Restoring workspace from backup snapshot following import failure"
    );
    copy_dir_contents(backup_dir, workspace, false)?;
    Ok(())
}

pub fn copy_dir_contents(src: &Path, dst: &Path, skip_ignored: bool) -> Result<(), ImportError> {
    let entries = fs::read_dir(src).map_err(|e| ImportError::IoError(e.to_string()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if skip_ignored
            && (name_str == ".dmz"
                || name_str == "target"
                || name_str == "dist"
                || name_str == "node_modules"
                || name_str == ".venv"
                || name_str == "venv"
                || name_str == "__pycache__")
        {
            continue;
        }

        let target = dst.join(&name);
        if path.is_dir() {
            fs::create_dir_all(&target).map_err(|e| ImportError::IoError(e.to_string()))?;
            copy_dir_contents(&path, &target, false)?;
        } else if path.is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| ImportError::IoError(e.to_string()))?;
            }
            fs::copy(&path, &target).map_err(|e| ImportError::IoError(e.to_string()))?;
        }
    }
    Ok(())
}

