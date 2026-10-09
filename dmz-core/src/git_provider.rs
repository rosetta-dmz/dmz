use git2::{Repository, Error as GitError};
use std::path::Path;

pub struct GitProvider;

impl GitProvider {
    /// Clones a repository offline/directly into a target workspace using embedded libgit2
    pub fn clone_repository(url: &str, target_path: &Path) -> Result<String, GitError> {
        let repo = Repository::clone(url, target_path)?;
        let head = repo.head()?;
        let commit_oid = head.target().ok_or_else(|| {
            GitError::from_str("Failed to resolve HEAD commit OID")
        })?;
        Ok(commit_oid.to_string())
    }

    /// Extracts the current commit hash of an existing local workspace repository
    pub fn get_current_commit(workspace_path: &Path) -> Result<String, GitError> {
        let repo = Repository::open(workspace_path)?;
        let head = repo.head()?;
        let commit = head.peel_to_commit()?;
        Ok(commit.id().to_string())
    }
}