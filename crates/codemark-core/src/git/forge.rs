//! Forge (git hosting provider) identification and display helpers.
//!
//! A "forge" is the platform hosting a repository (GitHub, GitLab, etc.). Forge
//! information is stored explicitly on auth accounts (`accounts.forge_kind`) and
//! can be inferred from a repository's `origin_url` host for repo owners.

/// A git hosting provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeKind {
    GitHub,
    GitLab,
    Bitbucket,
    /// An unknown or self-hosted forge that we can't classify.
    Unknown,
}

impl ForgeKind {
    /// Parse a stored `forge_kind` string (e.g. from `accounts.forge_kind`).
    pub fn from_forge_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "github" => ForgeKind::GitHub,
            "gitlab" => ForgeKind::GitLab,
            "bitbucket" => ForgeKind::Bitbucket,
            _ => ForgeKind::Unknown,
        }
    }

    /// Infer the forge from a git origin URL by inspecting its host.
    ///
    /// Supports HTTPS, SSH (`git@host:...`), and `ssh://` URLs. Falls back to
    /// [`ForgeKind::Unknown`] when the host doesn't match a known provider.
    pub fn from_origin_url(url: &str) -> Self {
        match host_from_url(url) {
            Some(host) => Self::from_host(&host),
            None => ForgeKind::Unknown,
        }
    }

    /// Classify a bare host name (e.g. `github.com`, `gitlab.example.com`).
    fn from_host(host: &str) -> Self {
        let host = host.to_ascii_lowercase();
        if host.contains("github") {
            ForgeKind::GitHub
        } else if host.contains("gitlab") {
            ForgeKind::GitLab
        } else if host.contains("bitbucket") {
            ForgeKind::Bitbucket
        } else {
            ForgeKind::Unknown
        }
    }

    /// A Nerd Font glyph identifying the forge.
    pub fn icon(self) -> &'static str {
        match self {
            ForgeKind::GitHub => "\u{f09b}",    //
            ForgeKind::GitLab => "\u{f296}",    //
            ForgeKind::Bitbucket => "\u{f171}", //
            ForgeKind::Unknown => "\u{e702}",   //  (generic git)
        }
    }

    /// A human-readable label for the forge.
    pub fn label(self) -> &'static str {
        match self {
            ForgeKind::GitHub => "GitHub",
            ForgeKind::GitLab => "GitLab",
            ForgeKind::Bitbucket => "Bitbucket",
            ForgeKind::Unknown => "Git",
        }
    }
}

/// Extract the host portion from a git URL (HTTPS, SSH, or `ssh://`).
fn host_from_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    // SSH shorthand: git@github.com:owner/repo.git
    if let Some(rest) = url.strip_prefix("git@")
        && let Some((host, _)) = rest.split_once(':')
    {
        return Some(host.to_string());
    }

    // Scheme-prefixed: https://host/..., http://host/..., ssh://git@host/...
    if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .or_else(|| url.strip_prefix("ssh://"))
    {
        // Drop any `user@` prefix, then take everything up to the first '/'.
        let rest = rest.rsplit_once('@').map(|(_, h)| h).unwrap_or(rest);
        let host = rest.split('/').next()?;
        // Strip an optional port.
        let host = host.split(':').next()?;
        if host.is_empty() {
            return None;
        }
        return Some(host.to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_forge_str_known() {
        assert_eq!(ForgeKind::from_forge_str("github"), ForgeKind::GitHub);
        assert_eq!(ForgeKind::from_forge_str("GitHub"), ForgeKind::GitHub);
        assert_eq!(ForgeKind::from_forge_str("gitlab"), ForgeKind::GitLab);
        assert_eq!(ForgeKind::from_forge_str("bitbucket"), ForgeKind::Bitbucket);
        assert_eq!(ForgeKind::from_forge_str("gitea"), ForgeKind::Unknown);
    }

    #[test]
    fn from_origin_url_https() {
        assert_eq!(
            ForgeKind::from_origin_url("https://github.com/owner/repo.git"),
            ForgeKind::GitHub
        );
        assert_eq!(
            ForgeKind::from_origin_url("https://gitlab.com/group/sub/repo.git"),
            ForgeKind::GitLab
        );
        assert_eq!(
            ForgeKind::from_origin_url("https://bitbucket.org/owner/repo"),
            ForgeKind::Bitbucket
        );
    }

    #[test]
    fn from_origin_url_ssh() {
        assert_eq!(ForgeKind::from_origin_url("git@github.com:owner/repo.git"), ForgeKind::GitHub);
        assert_eq!(
            ForgeKind::from_origin_url("ssh://git@gitlab.com/owner/repo.git"),
            ForgeKind::GitLab
        );
    }

    #[test]
    fn from_origin_url_self_hosted_gitlab() {
        assert_eq!(
            ForgeKind::from_origin_url("https://gitlab.example.com/owner/repo.git"),
            ForgeKind::GitLab
        );
    }

    #[test]
    fn from_origin_url_unknown_or_empty() {
        assert_eq!(ForgeKind::from_origin_url(""), ForgeKind::Unknown);
        assert_eq!(
            ForgeKind::from_origin_url("https://example.com/owner/repo"),
            ForgeKind::Unknown
        );
        assert_eq!(ForgeKind::from_origin_url("not a url"), ForgeKind::Unknown);
    }
}
