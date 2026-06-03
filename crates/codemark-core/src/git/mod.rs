pub mod context;
pub mod remote;

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
        tracing::debug!(target: "codemark::git", cmd = "rev-parse", repo = %repo, reference = %reference, "executing git command");

        let child = tokio::process::Command::new("git")
            .arg("rev-parse")
            .arg(reference)
            .current_dir(repo)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| crate::error::Error::Git(format!("git command failed: {e}")))?;

        let output =
            tokio::time::timeout(std::time::Duration::from_secs(10), child.wait_with_output())
                .await
                .map_err(|_| {
                    tracing::warn!(target: "codemark::git", repo = %repo, reference = %reference, "git rev-parse timed out");
                    crate::error::Error::Git("git command timed out".to_string())
                })?
                .map_err(|e| crate::error::Error::Git(format!("failed to wait for git: {e}")))?;

        if !output.status.success() {
            tracing::warn!(target: "codemark::git", repo = %repo, reference = %reference, "failed to resolve ref");
            return Err(crate::error::Error::Git(format!(
                "failed to resolve ref {}: {}",
                reference,
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        tracing::debug!(target: "codemark::git", sha = %sha, "resolved ref");
        Ok(sha)
    }

    async fn list_files(&self, repo: &str, commit: &str) -> Result<Vec<PathBuf>> {
        tracing::debug!(target: "codemark::git", cmd = "ls-tree", repo = %repo, commit = %commit, "executing git command");

        let child = tokio::process::Command::new("git")
            .arg("ls-tree")
            .arg("-r")
            .arg("--name-only")
            .arg(commit)
            .current_dir(repo)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| crate::error::Error::Git(format!("git command failed: {e}")))?;

        let output =
            tokio::time::timeout(std::time::Duration::from_secs(10), child.wait_with_output())
                .await
                .map_err(|_| {
                    tracing::warn!(target: "codemark::git", repo = %repo, commit = %commit, "git ls-tree timed out");
                    crate::error::Error::Git("git command timed out".to_string())
                })?
                .map_err(|e| crate::error::Error::Git(format!("failed to wait for git: {e}")))?;

        if !output.status.success() {
            tracing::warn!(target: "codemark::git", repo = %repo, commit = %commit, "failed to list files");
            return Err(crate::error::Error::Git(format!(
                "failed to list files for {}: {}",
                commit,
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let files: Vec<PathBuf> = String::from_utf8_lossy(&output.stdout).lines().map(PathBuf::from).collect();
        tracing::debug!(target: "codemark::git", count = files.len(), "listed files");
        Ok(files)
    }
}
