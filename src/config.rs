use std::collections::HashMap;
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
    #[serde(default)]
    pub open: OpenConfig,
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

/// Editor configuration for the `codemark open` command.
#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct OpenConfig {
    /// Default command template to use when no extension-specific override matches.
    /// Supports placeholders: {FILE}, {LINE_START}, {LINE_END}, {ID}
    pub default: Option<String>,
    /// Extension-specific command templates (e.g., "rs" -> "nvim +{LINE_START} {FILE}").
    pub extensions: HashMap<String, String>,
    /// Classification of editors by type (terminal vs GUI).
    pub editor_types: EditorTypesConfig,
}

/// Classification of editors by how they should be spawned.
#[derive(Debug, Deserialize, Serialize, Default)]
pub struct EditorTypesConfig {
    /// Terminal editors that take over the terminal and should be waited for.
    pub terminal: Vec<String>,
    /// GUI editors that spawn independently and return immediately.
    pub gui: Vec<String>,
}

impl Default for OpenConfig {
    fn default() -> Self {
        OpenConfig {
            default: None,
            extensions: HashMap::new(),
            editor_types: EditorTypesConfig::default(),
        }
    }
}

impl EditorTypesConfig {
    /// Default terminal editors that should block.
    fn default_terminal() -> Vec<String> {
        vec![
            "vim".to_string(),
            "vi".to_string(),
            "nvim".to_string(),
            "neovim".to_string(),
            "emacs".to_string(),
            "nano".to_string(),
            "micro".to_string(),
            "less".to_string(),
        ]
    }

    /// Default GUI editors that should spawn in background.
    fn default_gui() -> Vec<String> {
        vec![
            "xed".to_string(),
            "code".to_string(),
            "code-insiders".to_string(),
            "idea".to_string(),
            "subl".to_string(),
            "sublime".to_string(),
            "typora".to_string(),
            "atom".to_string(),
            "bbedit".to_string(),
            "textmate".to_string(),
        ]
    }

    /// Check if an editor program name is a terminal editor (should wait).
    pub fn is_terminal_editor(&self, program_name: &str) -> bool {
        // Check configured terminal list
        if self.terminal.iter().any(|e| e == program_name) {
            return true;
        }
        // Check if it's in the default terminal list
        Self::default_terminal().iter().any(|e| e == program_name)
    }

    /// Check if an editor program name is a GUI editor (should spawn in background).
    pub fn is_gui_editor(&self, program_name: &str) -> bool {
        // Check configured GUI list
        if self.gui.iter().any(|e| e == program_name) {
            return true;
        }
        // Check if it's in the default GUI list
        Self::default_gui().iter().any(|e| e == program_name)
    }
}

impl OpenConfig {
    /// Get the command for a specific file extension.
    /// Returns None if no extension-specific command is configured.
    pub fn get_command_for_extension(&self, extension: &str) -> Option<&String> {
        // Try case-sensitive match first
        if let Some(cmd) = self.extensions.get(extension) {
            return Some(cmd);
        }
        // Try case-insensitive match
        let lower_ext = extension.to_lowercase();
        for (key, cmd) in &self.extensions {
            if key.to_lowercase() == lower_ext {
                return Some(cmd);
            }
        }
        None
    }

    /// Check if an editor program name should block (wait for completion).
    /// Defaults to true for unknown editors (safer default).
    pub fn should_wait_for_editor(&self, program_name: &str) -> bool {
        if self.editor_types.is_gui_editor(program_name) {
            return false;
        }
        // If it's a known terminal editor or unknown, wait for it
        self.editor_types.is_terminal_editor(program_name) || !self.editor_types.is_gui_editor(program_name)
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
    fn parse_open_config_default() {
        let toml = r#"
[open]
default = "xed --line {LINE_START} {FILE}"

[open.extensions]
rs = "nvim +{LINE_START} {FILE}"
md = "typora {FILE}"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.open.default, Some("xed --line {LINE_START} {FILE}".to_string()));
        assert_eq!(config.open.extensions.get("rs"), Some(&"nvim +{LINE_START} {FILE}".to_string()));
        assert_eq!(config.open.extensions.get("md"), Some(&"typora {FILE}".to_string()));
    }

    #[test]
    fn editor_types_default_terminal_editors() {
        let config = EditorTypesConfig::default();
        assert!(config.is_terminal_editor("vim"));
        assert!(config.is_terminal_editor("nvim"));
        assert!(config.is_terminal_editor("emacs"));
        assert!(config.is_terminal_editor("nano"));
        assert!(!config.is_terminal_editor("code"));
        assert!(!config.is_terminal_editor("xed"));
    }

    #[test]
    fn editor_types_default_gui_editors() {
        let config = EditorTypesConfig::default();
        assert!(config.is_gui_editor("code"));
        assert!(config.is_gui_editor("xed"));
        assert!(config.is_gui_editor("idea"));
        assert!(config.is_gui_editor("typora"));
        assert!(!config.is_gui_editor("vim"));
        assert!(!config.is_gui_editor("nvim"));
    }

    #[test]
    fn editor_types_custom_override() {
        let toml = r#"
[open.editor_types]
terminal = ["myterm", "vim"]
gui = ["mygui", "code"]
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert!(config.open.editor_types.is_terminal_editor("myterm"));
        assert!(config.open.editor_types.is_gui_editor("mygui"));
    }

    #[test]
    fn should_wait_for_editor() {
        let config = OpenConfig::default();
        // Terminal editors - should wait
        assert!(config.should_wait_for_editor("vim"));
        assert!(config.should_wait_for_editor("nvim"));
        assert!(config.should_wait_for_editor("emacs"));
        // GUI editors - should not wait
        assert!(!config.should_wait_for_editor("code"));
        assert!(!config.should_wait_for_editor("xed"));
        // Unknown editors - safer to wait
        assert!(config.should_wait_for_editor("unknown-editor"));
    }

    #[test]
    fn parse_open_config_empty() {
        let toml = r#"
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.open.default, None);
        assert!(config.open.extensions.is_empty());
    }

    #[test]
    fn get_command_for_extension_case_insensitive() {
        let mut config = OpenConfig::default();
        config.extensions.insert("rs".to_string(), "nvim +{LINE_START} {FILE}".to_string());
        config.extensions.insert("md".to_string(), "typora {FILE}".to_string());

        // Case-sensitive match
        assert_eq!(
            config.get_command_for_extension("rs"),
            Some(&"nvim +{LINE_START} {FILE}".to_string())
        );
        // Case-insensitive match
        assert_eq!(
            config.get_command_for_extension("RS"),
            Some(&"nvim +{LINE_START} {FILE}".to_string())
        );
        // No match
        assert!(config.get_command_for_extension("py").is_none());
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
