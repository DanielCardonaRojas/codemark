//! Configuration info shown in the help overlay's Configuration tab.
//!
//! Surfaces the important on-disk locations and resolved settings a user (or an
//! agent helping them) might need: the active database, the layered config
//! files, data/templates/models directories, the log file, and the embeddings
//! model. Values are resolved fresh on each call so they reflect the active
//! database and the current layered configuration.

use std::path::Path;

use codemark_core::config::{self, Config};
use codemark_core::templates;

use crate::browser::BrowserLayout;

/// Render a path with the home directory collapsed to `~` for brevity.
///
/// Uses [`Path::strip_prefix`] (component-wise) rather than a string prefix so a
/// sibling like `/home/alice2` isn't mistaken for being under `/home/alice`.
fn abbreviate(path: &Path) -> String {
    if let Some(home) = config::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return if rest.as_os_str().is_empty() {
            "~".to_string()
        } else {
            format!("~/{}", rest.display())
        };
    }
    path.display().to_string()
}

impl BrowserLayout {
    /// Re-apply the active syntax theme to the right-pane code previews.
    ///
    /// The Settings overlay's Theme tab swaps the process-wide preview theme,
    /// but already-built previews captured the old one — this pushes the new
    /// theme into them so the code re-highlights in step with the chrome.
    pub fn reapply_preview_theme(&mut self) {
        self.right_pane.reapply_theme();
    }

    /// Build the labeled rows shown in the help overlay's Configuration tab.
    ///
    /// Each entry is a `(label, value)` pair; missing paths render as
    /// `<unavailable>` rather than being omitted, so the set of rows is stable.
    /// Paths under the home directory are abbreviated with `~`.
    pub fn config_info(&self) -> Vec<(&'static str, String, Option<String>)> {
        let db_path = self.db.path();
        let codemark_dir = db_path.parent();
        let config = codemark_dir.map(Config::load_layered).unwrap_or_default();

        let path_tuple = |p: Option<std::path::PathBuf>| {
            p.map(|p| (abbreviate(&p), Some(p.to_string_lossy().to_string())))
                .unwrap_or_else(|| ("<unavailable>".to_string(), None))
        };

        let registry_path = codemark_core::storage::registry::registry_path().ok();
        let registry_tuple = path_tuple(registry_path);

        let logs_path = std::env::temp_dir().join("codemark-tui.log");
        let logs = abbreviate(&std::env::temp_dir().join("codemark-tui.log.*"));

        let theme = config.tui.theme.clone().unwrap_or_else(|| "default".to_string());

        let global_config = path_tuple(config::global_config_dir().map(|d| d.join("config.toml")));
        let data_dir = path_tuple(config::global_data_dir());
        let templates_dir = path_tuple(templates::templates_dir());
        let grammars_dir = path_tuple(config::global_grammars_dir());

        let mut rows = vec![
            ("Version", crate::VERSION.to_string(), None),
            ("Database", abbreviate(db_path), Some(db_path.to_string_lossy().to_string())),
            ("Registry", registry_tuple.0, registry_tuple.1),
            ("Global config", global_config.0, global_config.1),
            ("Data dir", data_dir.0, data_dir.1),
            ("Templates", templates_dir.0, templates_dir.1),
            ("Grammars dir", grammars_dir.0, grammars_dir.1),
            ("Logs", logs, Some(logs_path.to_string_lossy().to_string())),
        ];

        // Semantic-search config only applies when the feature is compiled in.
        #[cfg(feature = "semantic")]
        {
            let model = config
                .semantic
                .model
                .clone()
                .unwrap_or_else(|| "all-MiniLM-L6-v2 (default)".to_string());
            let models_dir = path_tuple(config.semantic.get_models_dir());
            rows.push(("Models dir", models_dir.0, models_dir.1));
            rows.push(("Embeddings model", model, None));
        }

        rows.push(("TUI theme", theme, None));
        rows
    }
}
