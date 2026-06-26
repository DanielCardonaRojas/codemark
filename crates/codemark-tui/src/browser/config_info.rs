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
fn abbreviate(path: &Path) -> String {
    let display = path.display().to_string();
    if let Some(home) = config::home_dir() {
        let home = home.display().to_string();
        if let Some(rest) = display.strip_prefix(&home) {
            return format!("~{rest}");
        }
    }
    display
}

impl BrowserLayout {
    /// Build the labeled rows shown in the help overlay's Configuration tab.
    ///
    /// Each entry is a `(label, value)` pair; missing paths render as
    /// `<unavailable>` rather than being omitted, so the set of rows is stable.
    /// Paths under the home directory are abbreviated with `~`.
    pub fn config_info(&self) -> Vec<(&'static str, String)> {
        let db_path = self.db.path();
        let codemark_dir = db_path.parent();
        let config = codemark_dir.map(Config::load_layered).unwrap_or_default();

        // Render an optional path, falling back to a placeholder when the
        // platform dir can't be resolved.
        let path = |p: Option<std::path::PathBuf>| {
            p.map(|p| abbreviate(&p)).unwrap_or_else(|| "<unavailable>".to_string())
        };

        let mut rows: Vec<(&'static str, String)> = Vec::new();

        rows.push(("Version", crate::VERSION.to_string()));
        rows.push(("Database", abbreviate(db_path)));
        rows.push((
            "Registry",
            codemark_core::storage::registry::registry_path()
                .map(|p| abbreviate(&p))
                .unwrap_or_else(|_| "<unavailable>".to_string()),
        ));
        rows.push((
            "Global config",
            path(config::global_config_dir().map(|d| d.join("config.toml"))),
        ));
        rows.push(("Data dir", path(config::global_data_dir())));
        rows.push(("Templates", path(templates::templates_dir())));
        // The log writer rolls daily, so the live file is this prefix plus a
        // `.YYYY-MM-DD` suffix.
        rows.push(("Logs", abbreviate(&std::env::temp_dir().join("codemark-tui.log.*"))));
        rows.push(("Models dir", path(config.semantic.get_models_dir())));
        rows.push((
            "Embeddings model",
            config.semantic.model.clone().unwrap_or_else(|| "all-MiniLM-L6-v2 (default)".to_string()),
        ));
        rows.push((
            "TUI theme",
            config.tui.theme.clone().unwrap_or_else(|| "default".to_string()),
        ));

        rows
    }
}
