//! Git remote parsing utilities.

use crate::error::{Error, Result};
use std::path::Path;

/// Parse a Git remote URL into (owner, repo) parts.
///
/// Supports various Git URL formats:
/// - HTTPS: `https://github.com/owner/repo.git`
/// - SSH: `git@github.com:owner/repo.git`
/// - SSH with protocol: `ssh://git@github.com/owner/repo.git`
/// - Git protocol: `git://github.com/owner/repo.git`
///
/// Returns `Some((owner, repo))` if parsing succeeds, `None` otherwise.
pub fn parse_remote_url(url: &str) -> Option<(String, String)> {
    let url = url.trim();

    // Handle HTTPS URLs
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        return parse_github_url(rest);
    }

    // Handle HTTP URLs
    if let Some(rest) = url.strip_prefix("http://github.com/") {
        return parse_github_url(rest);
    }

    // Handle SSH URLs
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        return parse_github_url(rest);
    }
    if let Some(rest) = url.strip_prefix("git@github:") {
        return parse_github_url(rest);
    }

    // Handle SSH URLs with ssh:// protocol
    if let Some(rest) = url.strip_prefix("ssh://git@github.com/") {
        return parse_github_url(rest);
    }
    if let Some(rest) = url.strip_prefix("ssh://git@github/") {
        return parse_github_url(rest);
    }

    // Handle git:// protocol
    if let Some(rest) = url.strip_prefix("git://github.com/") {
        return parse_github_url(rest);
    }

    // Handle GitHub Enterprise (custom domains)
    if let Some(rest) = url.strip_prefix("https://") {
        if let Some(idx) = rest.find("/repos/") {
            let rest = &rest[idx + 7..]; // Skip "/repos/"
            return parse_github_url(rest);
        }
        if let Some(idx) = rest.find('/') {
            let rest = &rest[idx + 1..];
            return parse_github_url(rest);
        }
    }

    // Handle simple owner/repo format (for cases like --repo flag in tour list)
    if !url.contains("://") && !url.contains('@') && url.contains('/') {
        if let Some(idx) = url.find('/') {
            let owner = &url[..idx];
            let repo = &url[idx + 1..];
            if !owner.is_empty() && !repo.is_empty() {
                return Some((owner.to_string(), repo.to_string()));
            }
        }
    }

    None
}

/// Parse the path portion of a GitHub URL.
fn parse_github_url(path: &str) -> Option<(String, String)> {
    // Remove trailing slash first (so we can handle "owner/repo.git/")
    let path = path.trim_end_matches('/');
    // Remove .git suffix if present
    let path = path.strip_suffix(".git").unwrap_or(path);

    // Split into owner and repo
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() >= 2 {
        let owner = parts[0].to_string();
        let repo = parts[1].to_string();
        Some((owner, repo))
    } else {
        None
    }
}

/// Get the origin remote URL for a Git repository.
///
/// Runs `git remote get-url origin` in the specified directory.
pub fn get_origin_url(repo_path: &Path) -> Result<String> {
    let output = std::process::Command::new("git")
        .arg("remote")
        .arg("get-url")
        .arg("origin")
        .current_dir(repo_path)
        .output()
        .map_err(|e| Error::Git(format!("Failed to run git command: {}", e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Git(format!("Failed to get remote URL: {}", stderr)));
    }

    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(url)
}

/// Parse the current repository's origin remote into (owner, repo).
///
/// This is a convenience function that combines `get_origin_url` and `parse_remote_url`.
/// Supports GitHub.com and GitHub Enterprise URLs.
pub fn parse_current_repo(repo_path: &Path) -> Result<(String, String)> {
    let url = get_origin_url(repo_path)?;
    parse_remote_url(&url).ok_or_else(|| {
        Error::Git(format!(
            "Could not parse repository URL: {} (expected GitHub or GitHub Enterprise URL format)",
            url
        ))
    })
}

/// Get the GitHub URL from owner and repo parts.
pub fn build_github_url(owner: &str, repo: &str) -> String {
    format!("https://github.com/{}/{}.git", owner, repo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_https_url() {
        assert_eq!(
            parse_remote_url("https://github.com/owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_remote_url("https://github.com/owner/repo"),
            Some(("owner".to_string(), "repo".to_string()))
        );
    }

    #[test]
    fn test_parse_ssh_url() {
        assert_eq!(
            parse_remote_url("git@github.com:owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_remote_url("ssh://git@github.com/owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_remote_url("ssh://git@github/owner/repo.git"),
            Some(("owner".to_string(), "repo".to_string()))
        );
    }

    #[test]
    fn test_parse_with_nested_path() {
        assert_eq!(
            parse_remote_url("https://github.com/owner-org/repo-name.git"),
            Some(("owner-org".to_string(), "repo-name".to_string()))
        );
    }

    #[test]
    fn test_parse_with_trailing_slash() {
        assert_eq!(
            parse_remote_url("https://github.com/owner/repo/"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_remote_url("https://github.com/owner/repo.git/"),
            Some(("owner".to_string(), "repo".to_string()))
        );
    }

    #[test]
    fn test_parse_invalid_url() {
        assert_eq!(parse_remote_url("not-a-url"), None);
        assert_eq!(parse_remote_url("https://example.com/repo"), None);
    }

    #[test]
    fn test_parse_empty_string() {
        assert_eq!(parse_remote_url(""), None);
        assert_eq!(parse_remote_url("   "), None);
    }

    #[test]
    fn test_parse_owner_repo_format() {
        // Simple owner/repo format (used in --repo flag)
        assert_eq!(
            parse_remote_url("owner/repo"),
            Some(("owner".to_string(), "repo".to_string()))
        );
        assert_eq!(
            parse_remote_url("owner-org/repo-name"),
            Some(("owner-org".to_string(), "repo-name".to_string()))
        );
        // Edge cases
        assert_eq!(parse_remote_url("/repo"), None); // Missing owner
        assert_eq!(parse_remote_url("owner/"), None); // Missing repo
    }

    #[test]
    fn test_build_github_url() {
        assert_eq!(build_github_url("owner", "repo"), "https://github.com/owner/repo.git");
    }
}
