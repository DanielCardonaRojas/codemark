use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::embeddings::config::DistanceMetric;

/// Returns the global config directory.
///
/// Platform-specific locations:
/// - All platforms: `$XDG_CONFIG_HOME/codemark` if XDG_CONFIG_HOME is set
/// - macOS fallback: `~/Library/Application Support/codemark`
/// - Linux fallback: `~/.config/codemark` (XDG standard)
/// - Windows fallback: `%APPDATA%\codemark`
///
/// This is where the global `config.toml` file is stored.
pub fn global_config_dir() -> Option<PathBuf> {
    // Check XDG_CONFIG_HOME first (respects user preference on all platforms)
    if let Ok(xdg_config) = std::env::var("XDG_CONFIG_HOME") {
        let path = PathBuf::from(xdg_config).join("codemark");
        return Some(path);
    }

    // Use BaseDirs to avoid doubling the app name (ProjectDirs::from("", "codemark", "codemark")
    // would create "codemark-codemark" on macOS and "codemark\codemark" on Windows)
    directories::BaseDirs::new().map(|dirs| dirs.config_dir().join("codemark"))
}

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

    // Use BaseDirs to avoid doubling the app name
    directories::BaseDirs::new().map(|dirs| dirs.cache_dir().join("codemark").join("models"))
}

/// Application configuration.
///
/// Loaded from multiple sources with per-repo override:
/// 1. Global config: `~/.config/codemark/config.toml` (XDG standard)
/// 2. Local override (optional): `.codemark/config.toml` (repo-specific)
///
/// Local config values override global ones.
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
#[derive(Default)]
pub struct SemanticConfig {
    /// Whether semantic search is enabled (Some = explicitly set, None = use default).
    /// Use `is_enabled()` to get the resolved value.
    pub enabled: Option<bool>,
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

impl SemanticConfig {
    /// Get whether semantic search is enabled.
    /// Returns true if not explicitly set (default is enabled).
    pub fn is_enabled(&self) -> bool {
        self.enabled.unwrap_or(true)
    }

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
#[derive(Default)]
pub struct StorageConfig {
    /// Maximum resolution history entries to keep per bookmark.
    /// Older entries are pruned after each new resolution.
    /// Use `max_resolutions()` to get the resolved value (default: 20).
    pub max_resolutions_per_bookmark: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
#[derive(Default)]
pub struct HealthConfig {
    /// Days before stale bookmarks are auto-archived (used by `heal --auto-archive`).
    /// Use `auto_archive_days()` to get the resolved value (default: 7).
    pub auto_archive_after_days: Option<u32>,
}

impl StorageConfig {
    /// Get the maximum resolutions per bookmark (default: 20).
    pub fn max_resolutions(&self) -> usize {
        self.max_resolutions_per_bookmark.unwrap_or(20)
    }
}

impl HealthConfig {
    /// Get the auto-archive days threshold (default: 7).
    pub fn auto_archive_days(&self) -> u32 {
        self.auto_archive_after_days.unwrap_or(7)
    }
}

/// Editor configuration for the `codemark open` command.
#[derive(Debug, Default, Deserialize, Serialize)]
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
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct EditorTypesConfig {
    /// Terminal editors that take over the terminal and should be waited for.
    pub terminal: Vec<String>,
    /// GUI editors that spawn independently and return immediately.
    pub gui: Vec<String>,
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
            "helix".to_string(),
            "hx".to_string(),
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
        self.editor_types.is_terminal_editor(program_name)
            || !self.editor_types.is_gui_editor(program_name)
    }
}

impl Config {
    /// Load config from a `.codemark/config.toml` file. Returns defaults if the file doesn't exist.
    ///
    /// This loads a local (per-repo) config file only. For the full layered config
    /// (global + local override), use `Config::load_layered()`.
    #[allow(dead_code)]
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

    /// Load layered config: global config merged with local (per-repo) override.
    ///
    /// Loading strategy:
    /// 1. Always load global config from `~/.config/codemark/config.toml` (if it exists)
    /// 2. Load local config from `.codemark/config.toml` (if it exists)
    /// 3. Merge them, with local values taking precedence
    ///
    /// This allows users to have global defaults while customizing per-repo settings.
    pub fn load_layered(codemark_dir: &Path) -> Self {
        // Start with global config
        let mut config = Self::load_global();

        // Merge with local config if it exists
        let local_path = codemark_dir.join("config.toml");
        if let Ok(content) = std::fs::read_to_string(&local_path) {
            if let Ok(local) = toml::from_str::<Config>(&content) {
                // Merge: local values override global ones
                config.merge(local);
            } else {
                eprintln!(
                    "codemark: warning: invalid local config at {}: using global config only",
                    local_path.display()
                );
            }
        }

        config
    }

    /// Load global config from `~/.config/codemark/config.toml`.
    /// Returns defaults if the file doesn't exist.
    fn load_global() -> Self {
        let Some(config_dir) = global_config_dir() else {
            return Config::default();
        };

        let path = config_dir.join("config.toml");
        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).unwrap_or_else(|e| {
                eprintln!("codemark: warning: invalid global config at {}: {e}", path.display());
                Config::default()
            }),
            Err(_) => Config::default(),
        }
    }

    /// Merge another config into this one, with `other` taking precedence.
    /// Local config values override global ones only when explicitly set.
    fn merge(&mut self, other: Config) {
        // Storage config - override only if explicitly set in local
        if other.storage.max_resolutions_per_bookmark.is_some() {
            self.storage.max_resolutions_per_bookmark = other.storage.max_resolutions_per_bookmark;
        }
        // Health config - override only if explicitly set in local
        if other.health.auto_archive_after_days.is_some() {
            self.health.auto_archive_after_days = other.health.auto_archive_after_days;
        }

        // Semantic config - override only if explicitly set in local
        if other.semantic.enabled.is_some() {
            self.semantic.enabled = other.semantic.enabled;
        }
        if other.semantic.model.is_some() {
            self.semantic.model = other.semantic.model;
        }
        if other.semantic.models_dir.is_some() {
            self.semantic.models_dir = other.semantic.models_dir;
        }
        if other.semantic.batch_size.is_some() {
            self.semantic.batch_size = other.semantic.batch_size;
        }
        if other.semantic.distance_metric.is_some() {
            self.semantic.distance_metric = other.semantic.distance_metric;
        }
        if other.semantic.threshold.is_some() {
            self.semantic.threshold = other.semantic.threshold;
        }

        // Open config merge
        if other.open.default.is_some() {
            self.open.default = other.open.default;
        }
        // Merge extensions (local extends global)
        for (key, value) in other.open.extensions {
            self.open.extensions.insert(key, value);
        }
        // Merge editor types (extend, don't replace)
        for editor in other.open.editor_types.terminal {
            if !self.open.editor_types.terminal.contains(&editor) {
                self.open.editor_types.terminal.push(editor);
            }
        }
        for editor in other.open.editor_types.gui {
            if !self.open.editor_types.gui.contains(&editor) {
                self.open.editor_types.gui.push(editor);
            }
        }
    }

    /// Write the default config file to the global config directory.
    /// This creates a new config file with helpful comments explaining each option.
    /// Returns Ok(true) if the file was created, Ok(false) if it already exists.
    pub fn init_global_default() -> std::io::Result<bool> {
        let Some(config_dir) = global_config_dir() else {
            return Ok(false);
        };

        // Create config directory if it doesn't exist
        std::fs::create_dir_all(&config_dir)?;

        let path = config_dir.join("config.toml");

        // Don't overwrite existing config
        if path.exists() {
            return Ok(false);
        }

        let default_content = include_str!("../docs/config.default.toml");
        std::fs::write(&path, default_content)?;
        Ok(true)
    }

    /// Write the default config file to the `.codemark` directory.
    /// This creates a new config file with helpful comments explaining each option.
    /// Returns Ok(true) if the file was created, Ok(false) if it already exists.
    ///
    /// Deprecated: Use `init_global_default()` for the new global config location.
    /// This method is kept for backward compatibility.
    #[allow(dead_code)]
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
        assert_eq!(config.storage.max_resolutions(), 20);
        assert_eq!(config.health.auto_archive_days(), 7);
        assert_eq!(config.semantic.is_enabled(), true);
    }

    #[test]
    fn parse_partial_config() {
        let toml = r#"
[storage]
max_resolutions_per_bookmark = 5
"#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.storage.max_resolutions(), 5);
        // Health defaults preserved
        assert_eq!(config.health.auto_archive_days(), 7);
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
        assert_eq!(config.storage.max_resolutions(), 10);
        assert_eq!(config.health.auto_archive_days(), 14);
    }

    #[test]
    fn load_missing_file_returns_defaults() {
        let config = Config::load(Path::new("/nonexistent/path"));
        assert_eq!(config.storage.max_resolutions(), 20);
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
        assert_eq!(
            config.open.extensions.get("rs"),
            Some(&"nvim +{LINE_START} {FILE}".to_string())
        );
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
        assert_eq!(config.storage.max_resolutions(), 20);
        assert_eq!(config.semantic.is_enabled(), true);

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

    #[test]
    fn layered_config_merges_global_and_local() {
        // This test verifies that local config overrides global config
        let global_toml = r#"
[storage]
max_resolutions_per_bookmark = 10

[open]
default = "vim {FILE}"

[open.extensions]
rs = "nvim +{LINE_START} {FILE}"
md = "typora {FILE}"
"#;

        let local_toml = r#"
[storage]
max_resolutions_per_bookmark = 5

[open.extensions]
swift = "xed --line {LINE_START} {FILE}"
py = "code {FILE}"
"#;

        let mut global: Config = toml::from_str(global_toml).unwrap();
        let local: Config = toml::from_str(local_toml).unwrap();

        // Merge local into global
        global.merge(local);

        // Local storage override should win
        assert_eq!(global.storage.max_resolutions(), 5);

        // Global default should remain
        assert_eq!(global.open.default, Some("vim {FILE}".to_string()));

        // Extensions should be merged (both global and local present)
        assert_eq!(
            global.open.extensions.get("rs"),
            Some(&"nvim +{LINE_START} {FILE}".to_string())
        );
        assert_eq!(global.open.extensions.get("md"), Some(&"typora {FILE}".to_string()));
        assert_eq!(
            global.open.extensions.get("swift"),
            Some(&"xed --line {LINE_START} {FILE}".to_string())
        );
        assert_eq!(global.open.extensions.get("py"), Some(&"code {FILE}".to_string()));
    }

    #[test]
    fn layered_config_defaults_preserved() {
        // When local config doesn't specify a value, global defaults are used
        let global_toml = r#"
[storage]
max_resolutions_per_bookmark = 15

[semantic]
enabled = true
"#;

        let local_toml = r#"
[health]
auto_archive_after_days = 14
"#;

        let mut global: Config = toml::from_str(global_toml).unwrap();
        let local: Config = toml::from_str(local_toml).unwrap();

        global.merge(local);

        // Global values preserved
        assert_eq!(global.storage.max_resolutions(), 15);
        assert_eq!(global.semantic.is_enabled(), true);

        // Local value applied
        assert_eq!(global.health.auto_archive_days(), 14);
    }

    #[test]
    fn layered_config_local_default_values_override_global() {
        // Test that local values equal to defaults still override global
        let global_toml = r#"
[storage]
max_resolutions_per_bookmark = 100

[semantic]
enabled = false
"#;

        let local_toml = r#"
[storage]
max_resolutions_per_bookmark = 20

[semantic]
enabled = true
"#;

        let mut global: Config = toml::from_str(global_toml).unwrap();
        let local: Config = toml::from_str(local_toml).unwrap();

        global.merge(local);

        // Local values (equal to defaults) should win over global
        assert_eq!(global.storage.max_resolutions(), 20);
        assert_eq!(global.semantic.is_enabled(), true);
    }

    #[test]
    fn default_config_template_parses_correctly() {
        // Regression test: ensure the embedded default config template parses correctly
        let default_content = include_str!("../docs/config.default.toml");
        let config: Config = toml::from_str(default_content)
            .expect("Default config template should parse correctly");
        assert_eq!(config.storage.max_resolutions(), 20);
        assert_eq!(config.health.auto_archive_days(), 7);
        assert_eq!(config.semantic.is_enabled(), true);
        assert_eq!(config.open.default, Some("vim +{LINE_START} {FILE}".to_string()));
        assert!(config.open.extensions.contains_key("rs"));
        assert!(config.open.extensions.contains_key("swift"));
        assert!(config.open.extensions.contains_key("py"));
    }

    #[test]
    fn editor_types_merge_extends_not_replaces() {
        // Editor type lists should extend, not replace
        let global_toml = r#"
[open.editor_types]
terminal = ["vim", "nvim"]
gui = ["code"]
"#;

        let local_toml = r#"
[open.editor_types]
terminal = ["emacs"]
gui = ["xed", "idea"]
"#;

        let mut global: Config = toml::from_str(global_toml).unwrap();
        let local: Config = toml::from_str(local_toml).unwrap();

        global.merge(local);

        // Terminal editors should be merged
        assert_eq!(global.open.editor_types.terminal.len(), 3);
        assert!(global.open.editor_types.terminal.contains(&"vim".to_string()));
        assert!(global.open.editor_types.terminal.contains(&"nvim".to_string()));
        assert!(global.open.editor_types.terminal.contains(&"emacs".to_string()));

        // GUI editors should be merged
        assert_eq!(global.open.editor_types.gui.len(), 3);
        assert!(global.open.editor_types.gui.contains(&"code".to_string()));
        assert!(global.open.editor_types.gui.contains(&"xed".to_string()));
        assert!(global.open.editor_types.gui.contains(&"idea".to_string()));
    }

    #[test]
    fn editor_types_merge_deduplicates() {
        // Duplicate editor names should not be added twice
        let global_toml = r#"
[open.editor_types]
terminal = ["vim", "nvim"]
"#;

        let local_toml = r#"
[open.editor_types]
terminal = ["vim", "emacs"]
"#;

        let mut global: Config = toml::from_str(global_toml).unwrap();
        let local: Config = toml::from_str(local_toml).unwrap();

        global.merge(local);

        // vim should appear only once
        assert_eq!(global.open.editor_types.terminal.len(), 3);
        let vim_count =
            global.open.editor_types.terminal.iter().filter(|x| x.as_str() == "vim").count();
        assert_eq!(vim_count, 1);
    }
}
