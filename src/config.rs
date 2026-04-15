use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::embeddings::config::DistanceMetric;

/// Returns the global models cache directory.
///
/// Platform-specific locations:
/// - macOS: `~/Library/Caches/codemark`
/// - Linux: `~/.cache/codemark` (XDG standard)
/// - Windows: `%LOCALAPPDATA%\codemark\cache`
///
/// Can be overridden by the `CODMARK_MODELS_DIR` environment variable.
pub fn global_models_dir() -> Option<PathBuf> {
    // Check environment variable first
    if let Ok(env_dir) = std::env::var("CODMARK_MODELS_DIR") {
        return Some(PathBuf::from(env_dir));
    }

    // Use platform-specific cache directory
    directories::ProjectDirs::from("", "codemark", "codemark")
        .map(|proj| proj.cache_dir().join("models"))
}

/// Application configuration loaded from `.codemark/config.toml`.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub health: HealthConfig,
    #[serde(default)]
    pub semantic: SemanticConfig,
}

/// Semantic search configuration wrapper.
#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct SemanticConfig {
    pub enabled: bool,
    pub model: Option<String>,
    /// Directory for storing embedding models.
    /// If not set, uses the global cache directory.
    /// Can also be set via `CODMARK_MODELS_DIR` environment variable.
    pub models_dir: Option<String>,
    pub batch_size: Option<usize>,
    /// Distance metric for similarity search (l2, cosine, ip).
    pub distance_metric: Option<String>,
    /// Maximum distance for a match (None = no threshold).
    /// For l2/cosine: values <= threshold are matches.
    /// For ip (inner product): values >= threshold are matches.
    pub threshold: Option<f32>,
}

impl Default for SemanticConfig {
    fn default() -> Self {
        SemanticConfig {
            enabled: true,
            model: None,
            models_dir: None,
            batch_size: None,
            distance_metric: None,
            threshold: None,
        }
    }
}

impl SemanticConfig {
    /// Parse the distance metric from the string config.
    pub fn get_distance_metric(&self) -> DistanceMetric {
        self.distance_metric.as_ref().and_then(|s| s.parse().ok()).unwrap_or_default()
    }

    /// Get the effective models directory.
    ///
    /// Returns the configured directory if set, otherwise the global cache.
    pub fn get_models_dir(&self) -> Option<PathBuf> {
        if let Some(dir) = &self.models_dir {
            // Expand ~ to home directory
            let expanded = shellexpand::tilde(dir);
            Some(PathBuf::from(expanded.as_ref()))
        } else {
            global_models_dir()
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct StorageConfig {
    /// Maximum resolution history entries to keep per bookmark.
    /// Older entries are pruned after each new resolution.
    pub max_resolutions_per_bookmark: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct HealthConfig {
    /// Days before stale bookmarks are auto-archived (used by `heal --auto-archive`).
    pub auto_archive_after_days: u32,
}

impl Default for StorageConfig {
    fn default() -> Self {
        StorageConfig { max_resolutions_per_bookmark: 20 }
    }
}

impl Default for HealthConfig {
    fn default() -> Self {
        HealthConfig { auto_archive_after_days: 7 }
    }
}

impl Config {
    /// Load config from a `.codemark/config.toml` file. Returns defaults if the file doesn't exist.
    pub fn load(codemark_dir: &Path) -> Self {
        let path = codemark_dir.join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
                eprintln!("codemark: warning: invalid config at {}: {e}", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    /// Write the default config file to the `.codemark` directory.
    /// This creates a new config file with helpful comments explaining each option.
    /// Returns Ok(true) if the file was created, Ok(false) if it already exists.
    pub fn init_default(codemark_dir: &Path) -> std::io::Result<bool> {
        let path = codemark_dir.join("config.toml");

        // Don't overwrite existing config
        if path.exists() {
            return Ok(false);
        }

        let default_content = include_str!("../docs/config.default.toml");
        std::fs::write(&path, default_content)?;
        Ok(true)
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
        assert_eq!(config.semantic.enabled, true);
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

    #[test]
    fn parse_semantic_config_with_threshold() {
        let toml = r#"
[semantic]
enabled = true
distance_metric = "cosine"
threshold = 0.4
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.semantic.threshold, Some(0.4));
        assert_eq!(config.semantic.distance_metric, Some("cosine".to_string()));
        assert_eq!(config.semantic.get_distance_metric(), DistanceMetric::Cosine);
    }

    #[test]
    fn parse_semantic_config_with_l2_metric() {
        let toml = r#"
[semantic]
distance_metric = "l2"
threshold = 0.5
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.semantic.get_distance_metric(), DistanceMetric::L2);
    }

    #[test]
    fn parse_semantic_config_with_inner_product() {
        let toml = r#"
[semantic]
distance_metric = "ip"
threshold = 0.8
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.semantic.get_distance_metric(), DistanceMetric::InnerProduct);
        assert_eq!(config.semantic.threshold, Some(0.8));
    }

    #[test]
    fn init_default_creates_config_file() {
        let tmp = std::env::temp_dir().join("codemark_test_config_init");
        let _ = std::fs::create_dir_all(&tmp);

        let config_path = tmp.join("config.toml");

        // Ensure clean state
        let _ = std::fs::remove_file(&config_path);

        // First call should create the file
        let created = Config::init_default(&tmp).unwrap();
        assert!(created);
        assert!(config_path.exists());

        // Verify the config can be loaded
        let config = Config::load(&tmp);
        assert_eq!(config.storage.max_resolutions_per_bookmark, 20);
        assert_eq!(config.semantic.enabled, true);

        // Second call should not overwrite
        let created_again = Config::init_default(&tmp).unwrap();
        assert!(!created_again);

        // Cleanup
        let _ = std::fs::remove_file(&config_path);
        let _ = std::fs::remove_dir(&tmp);
    }

    #[test]
    fn init_default_skips_existing_file() {
        let tmp = std::env::temp_dir().join("codemark_test_config_skip");
        let _ = std::fs::create_dir_all(&tmp);

        let config_path = tmp.join("config.toml");

        // Create a custom config
        let custom_content = r#"[storage]
max_resolutions_per_bookmark = 99
"#;
        std::fs::write(&config_path, custom_content).unwrap();

        // Should not overwrite
        let created = Config::init_default(&tmp).unwrap();
        assert!(!created);

        // Verify custom config is preserved
        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("99"));
        assert!(!content.contains("# Codemark Configuration"));

        // Cleanup
        let _ = std::fs::remove_file(&config_path);
        let _ = std::fs::remove_dir(&tmp);
    }
}
