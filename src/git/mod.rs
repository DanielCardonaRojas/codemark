pub mod context;

use crate::error::Result;
use async_trait::async_trait;
use std::path::PathBuf;

#[async_trait]
pub trait GitProvider: Send + Sync {
    /// Resolves a reference (branch, tag, HEAD) to a full 40-char SHA.
    async fn resolve_ref(&self, repo: &str, reference: &str) -> Result<String>;

    /// Lists files in a tree at a specific commit.
    async fn list_files(&self, repo: &str, commit: &str) -> Result<Vec<PathBuf>>;
}

pub struct LocalGitProvider;

#[async_trait]
impl GitProvider for LocalGitProvider {
    async fn resolve_ref(&self, repo: &str, reference: &str) -> Result<String> {
        let output = tokio::process::Command::new("git")
            .arg("rev-parse")
            .arg(reference)
            .current_dir(repo)
            .output()
            .await
            .map_err(|e| crate::error::Error::Git(format!("git command failed: {e}")))?;

        if !output.status.success() {
            return Err(crate::error::Error::Git(format!(
                "failed to resolve ref {}: {}",
                reference,
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn list_files(&self, repo: &str, commit: &str) -> Result<Vec<PathBuf>> {
        let output = tokio::process::Command::new("git")
            .arg("ls-tree")
            .arg("-r")
            .arg("--name-only")
            .arg(commit)
            .current_dir(repo)
            .output()
            .await
            .map_err(|e| crate::error::Error::Git(format!("git command failed: {e}")))?;

        if !output.status.success() {
            return Err(crate::error::Error::Git(format!(
                "failed to list files for {}: {}",
                commit,
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let files = String::from_utf8_lossy(&output.stdout).lines().map(PathBuf::from).collect();

        Ok(files)
    }
}
