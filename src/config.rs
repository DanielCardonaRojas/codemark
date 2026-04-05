use std::path::Path;

use serde::Deserialize;

/// Application configuration loaded from `.codemark/config.toml`.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub storage: StorageConfig,
    pub health: HealthConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Maximum resolution history entries to keep per bookmark.
    /// Older entries are pruned after each new resolution.
    pub max_resolutions_per_bookmark: usize,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct HealthConfig {
    /// Days before stale bookmarks are auto-archived (used by `validate --auto-archive`).
    pub auto_archive_after_days: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            storage: StorageConfig::default(),
            health: HealthConfig::default(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageConfig {
            max_resolutions_per_bookmark: 20,
        }
    }
}

impl Default for HealthConfig {
    fn default() -> Self {
        HealthConfig {
            auto_archive_after_days: 7,
        }
    }
}

impl Config {
    /// Load config from a `.codemark/config.toml` file. Returns defaults if the file doesn't exist.
    pub fn load(codemark_dir: &Path) -> Self {
        let path = codemark_dir.join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
                eprintln!(
                    "codemark: warning: invalid config at {}: {e}",
                    path.display()
                );
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let config = Config::default();
        assert_eq!(config.storage.max_resolutions_per_bookmark, 20);
        assert_eq!(config.health.auto_archive_after_days, 7);
    }

    #[test]
    fn parse_partial_config() {
        let toml = r#"
[storage]
max_resolutions_per_bookmark = 5
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.storage.max_resolutions_per_bookmark, 5);
        // Health defaults preserved
        assert_eq!(config.health.auto_archive_after_days, 7);
    }

    #[test]
    fn parse_full_config() {
        let toml = r#"
[storage]
max_resolutions_per_bookmark = 10

[health]
auto_archive_after_days = 14
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.storage.max_resolutions_per_bookmark, 10);
        assert_eq!(config.health.auto_archive_after_days, 14);
    }

    #[test]
    fn load_missing_file_returns_defaults() {
        let config = Config::load(Path::new("/nonexistent/path"));
        assert_eq!(config.storage.max_resolutions_per_bookmark, 20);
    }
}
