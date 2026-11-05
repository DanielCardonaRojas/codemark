//! The settings overlay's **Languages** tab: browse the grammar registry and
//! install grammars without leaving the TUI.
//!
//! Since 0.8.0 most languages are WASM grammars installed from the registry, so
//! a TUI-only user would otherwise have to drop to the CLI
//! (`codemark languages install <lang>`) the first time a repo's language shows
//! "No grammar".
//!
//! The tab owns only view state. Network work — fetching the index, downloading
//! and installing a grammar — is requested through [`SettingsAction`] and run on
//! a background task by the main loop, which reports back through
//! [`on_index_loaded`](LanguagesTab::on_index_loaded) and
//! [`on_installed`](LanguagesTab::on_installed).

use std::collections::HashMap;

use codemark_core::grammar::registry_source::RegistryIndex;
use codemark_core::parser::languages::{GrammarIdentity, Language};
use crossterm::event::KeyCode;
use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget},
};

use crate::settings::SettingsAction;

/// Which section of the tab a row belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Grammars that can parse right now: compiled-in or in the grammar cache.
    Installed,
    /// Registry entries that are not installed.
    Available,
}

/// A grammar codemark can already parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledGrammar {
    pub name: String,
    pub extensions: Vec<String>,
    /// Compiled into the binary rather than loaded from the grammar cache.
    pub builtin: bool,
    /// The registry profile version recorded at install time, if any. Read
    /// from the installed manifest, never from the registry's current index —
    /// that would make an outdated install look current.
    pub profile_version: Option<String>,
}

impl InstalledGrammar {
    /// Everything parseable right now: the built-ins plus whatever the global
    /// grammar registry has discovered in the cache.
    pub fn current() -> Vec<Self> {
        Language::builtins()
            .into_iter()
            .chain(codemark_core::parser::registry::LanguageRegistry::dynamic_languages())
            .map(|lang| InstalledGrammar {
                name: lang.name().to_string(),
                extensions: lang.file_extensions(),
                builtin: lang.identity() == GrammarIdentity::Builtin,
                profile_version: match &lang {
                    Language::Dynamic(dl) => codemark_core::grammar::installed_profile_version(dl),
                    _ => None,
                },
            })
            .collect()
    }
}

/// One selectable line in the tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub extensions: Vec<String>,
    pub section: Section,
    /// Short trailing detail: `built-in`, a profile version, a description.
    pub note: String,
    /// Whether `Enter` can install it: available, with a prebuilt grammar.
    pub installable: bool,
}

/// Build the rows: installed grammars first, then registry entries not yet
/// installed, each section sorted by name.
///
/// A dynamic grammar that overrides a built-in of the same name appears once,
/// as the grammar actually in use. An installed grammar counts against a
/// registry entry by name, case-insensitively.
pub fn build_rows(installed: &[InstalledGrammar], index: Option<&RegistryIndex>) -> Vec<Row> {
    // A dynamic grammar wins over a built-in of the same name, so list that one.
    let mut by_name: HashMap<String, &InstalledGrammar> = HashMap::new();
    for g in installed {
        let key = g.name.to_lowercase();
        match by_name.get(&key) {
            Some(existing) if !existing.builtin => {}
            _ => {
                by_name.insert(key, g);
            }
        }
    }
    let mut installed_rows: Vec<Row> = by_name
        .values()
        .map(|g| {
            let note = match (&g.profile_version, g.builtin) {
                (_, true) => "built-in".to_string(),
                (Some(v), false) => format!("profile {v}"),
                (None, false) => String::new(),
            };
            Row {
                name: g.name.clone(),
                extensions: g.extensions.clone(),
                section: Section::Installed,
                note,
                installable: false,
            }
        })
        .collect();
    installed_rows.sort_by_key(|r| r.name.to_lowercase());

    let mut available_rows: Vec<Row> = index
        .map(|ix| ix.grammars.as_slice())
        .unwrap_or_default()
        .iter()
        .filter(|e| !by_name.contains_key(&e.name.to_lowercase()))
        .map(|e| {
            let installable = e.wasm_url.is_some();
            let note = if installable {
                e.description.clone().unwrap_or_default()
            } else {
                "manual build".to_string()
            };
            Row {
                name: e.name.clone(),
                extensions: e.extensions.clone(),
                section: Section::Available,
                note,
                installable,
            }
        })
        .collect();
    available_rows.sort_by_key(|r| r.name.to_lowercase());

    installed_rows.into_iter().chain(available_rows).collect()
}

/// Where the registry index is in its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
enum IndexState {
    /// Not requested yet; opening the tab requests it.
    NotLoaded,
    Loading,
    Loaded(RegistryIndex),
    /// No index at all — not even a cached one. The installed section still
    /// renders.
    Failed(String),
}

/// State for the Languages tab.
pub struct LanguagesTab {
    index: IndexState,
    /// A fetch is in flight. A loaded index stays on screen meanwhile, so a
    /// refresh doesn't hide the Available rows (and the selection with them).
    fetching: bool,
    /// The grammar being installed, if any. One at a time keeps the cache's
    /// install lock uncontended and the UI simple.
    installing: Option<String>,
    /// Last install error per grammar, shown under its row until retried.
    errors: HashMap<String, String>,
    selected: usize,
    /// Installed grammars, cached: re-read when the tab is shown and after an
    /// install rather than on every frame (it reads each grammar's manifest).
    installed: Vec<InstalledGrammar>,
    /// Source of installed grammars; swapped out in tests so they don't depend
    /// on the process-global grammar registry.
    installed_source: fn() -> Vec<InstalledGrammar>,
}

impl Default for LanguagesTab {
    fn default() -> Self {
        Self::new()
    }
}

impl LanguagesTab {
    pub fn new() -> Self {
        Self::with_installed(InstalledGrammar::current)
    }

    fn with_installed(installed_source: fn() -> Vec<InstalledGrammar>) -> Self {
        Self {
            index: IndexState::NotLoaded,
            fetching: false,
            installing: None,
            errors: HashMap::new(),
            selected: 0,
            installed: installed_source(),
            installed_source,
        }
    }

    /// Re-read the installed grammars (e.g. one added from the CLI meanwhile).
    fn refresh_installed(&mut self) {
        self.installed = (self.installed_source)();
    }

    /// The grammar cache changed outside the tab — the terminal regained focus
    /// after a `codemark languages add`/`remove`, say. Re-read what is
    /// installed so a removed grammar stops looking installed.
    pub fn on_grammars_changed(&mut self) {
        self.refresh_installed();
        self.clamp_selection();
    }

    /// The current rows, recomputed so an install (or a grammar added from the
    /// CLI meanwhile) shows up without extra bookkeeping.
    pub fn rows(&self) -> Vec<Row> {
        let index = match &self.index {
            IndexState::Loaded(ix) => Some(ix),
            _ => None,
        };
        build_rows(&self.installed, index)
    }

    /// Request the index the first time the tab is shown. Returns the action
    /// for the main loop to run, or `None` when it is loaded or in flight.
    pub fn on_shown(&mut self) -> Option<SettingsAction> {
        self.refresh_installed();
        self.clamp_selection();
        if self.index == IndexState::NotLoaded {
            self.index = IndexState::Loading;
            self.fetching = true;
            return Some(SettingsAction::FetchRegistry { refresh: false });
        }
        None
    }

    /// Apply a finished index fetch.
    pub fn on_index_loaded(&mut self, result: Result<RegistryIndex, String>) {
        // Keep the user on the same grammar across a refresh: rows can shift
        // when the registry adds or drops an entry.
        let was_on = self.rows().get(self.selected).map(|r| r.name.clone());
        self.fetching = false;
        self.index = match result {
            Ok(index) => IndexState::Loaded(index),
            // A failed *refresh* keeps the list it already had; the failure is
            // only worth a screen of its own when there is nothing to show.
            Err(_) if matches!(self.index, IndexState::Loaded(_)) => {
                return self.clamp_selection();
            }
            Err(e) => IndexState::Failed(e),
        };
        if let Some(pos) = was_on.and_then(|n| self.rows().iter().position(|r| r.name == n)) {
            self.selected = pos;
        }
        self.clamp_selection();
    }

    /// Apply a finished install. On success the grammar has already been
    /// picked up by the global registry (the main loop refreshes it first), so
    /// it moves to the Installed section; keep it selected there.
    pub fn on_installed(&mut self, name: &str, result: Result<(), String>) {
        if self.installing.as_deref() == Some(name) {
            self.installing = None;
        }
        match result {
            Ok(()) => {
                self.refresh_installed();
                self.errors.remove(name);
                if let Some(pos) = self.rows().iter().position(|r| r.name == name) {
                    self.selected = pos;
                }
            }
            Err(e) => {
                self.errors.insert(name.to_string(), e);
            }
        }
        self.clamp_selection();
    }

    fn clamp_selection(&mut self) {
        let len = self.rows().len();
        self.selected = self.selected.min(len.saturating_sub(1));
    }

    /// Handle a key while the tab is active.
    pub fn handle_key(&mut self, code: KeyCode) -> SettingsAction {
        let rows = self.rows();
        match code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected + 1 < rows.len() {
                    self.selected += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Char('r') => {
                // Refresh both halves: what is installed may have changed too.
                self.on_grammars_changed();
                if !self.fetching {
                    self.fetching = true;
                    // Keep a loaded index visible; only replace a failure or
                    // nothing-yet with the loading line.
                    if !matches!(self.index, IndexState::Loaded(_)) {
                        self.index = IndexState::Loading;
                    }
                    return SettingsAction::FetchRegistry { refresh: true };
                }
            }
            KeyCode::Enter => {
                let Some(row) = rows.get(self.selected) else {
                    return SettingsAction::Handled;
                };
                if row.section == Section::Installed {
                    return SettingsAction::Handled;
                }
                if !row.installable {
                    return SettingsAction::Notify(format!(
                        "{} has no prebuilt grammar — build it, then run `codemark languages add \
                         <grammar.wasm> --from-registry {}`",
                        row.name, row.name
                    ));
                }
                if !cfg!(feature = "wasm") {
                    return SettingsAction::Notify(
                        "This build of codemark can't load WASM grammars (built without the \
                         `wasm` feature), so installing one would have no effect"
                            .to_string(),
                    );
                }
                if let Some(busy) = &self.installing {
                    return SettingsAction::Notify(format!("Still installing {busy}"));
                }
                self.installing = Some(row.name.clone());
                self.errors.remove(&row.name);
                return SettingsAction::InstallGrammar(row.name.clone());
            }
            _ => {}
        }
        SettingsAction::Handled
    }

    /// Render the tab body.
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let palette = crate::theme::palette();
        let rows = self.rows();

        let mut text = Text::default();
        match &self.index {
            IndexState::NotLoaded | IndexState::Loading => text.push_line(Line::styled(
                " Fetching the grammar registry…",
                Style::default().fg(palette.dim),
            )),
            IndexState::Failed(e) => text.push_line(Line::styled(
                format!(" Could not load the grammar registry: {e}"),
                Style::default().fg(palette.error),
            )),
            IndexState::Loaded(_) if self.fetching => text.push_line(Line::styled(
                " Refreshing the grammar registry…",
                Style::default().fg(palette.dim),
            )),
            IndexState::Loaded(ix) => match &ix.stale {
                Some(e) => text.push_line(Line::styled(
                    format!(" Could not load the registry — showing the cached list ({e})"),
                    Style::default().fg(palette.warning),
                )),
                None => text.push_line(Line::styled(
                    format!(" {} grammars in the registry", ix.grammars.len()),
                    Style::default().fg(palette.dim),
                )),
            },
        }

        let name_width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
        let ext_width = rows.iter().map(|r| ext_label(r).len()).max().unwrap_or(0);

        // Line index of the selected row, so the view can scroll to keep it visible.
        let mut selected_line = 0;
        let mut section = None;
        for (i, row) in rows.iter().enumerate() {
            if section != Some(row.section) {
                section = Some(row.section);
                text.push_line(Line::raw(""));
                let title = match row.section {
                    Section::Installed => "Installed",
                    Section::Available => "Available",
                };
                text.push_line(Line::styled(
                    format!(" {title}"),
                    Style::default().fg(palette.accent).bold(),
                ));
            }

            let selected = i == self.selected;
            if selected {
                selected_line = text.lines.len();
            }
            let marker = if selected {
                Span::styled("  \u{276f} ", Style::default().fg(palette.marker).bold())
            } else {
                Span::raw("    ")
            };
            let name_style = match (selected, row.section, row.installable) {
                (true, _, _) => Style::default().fg(palette.emphasis).bold(),
                (false, Section::Available, false) => Style::default().fg(palette.gray),
                _ => Style::default().fg(palette.emphasis),
            };
            let status = if self.installing.as_deref() == Some(row.name.as_str()) {
                Span::styled("installing…", Style::default().fg(palette.accent).bold())
            } else {
                Span::styled(row.note.clone(), Style::default().fg(palette.dim))
            };
            text.push_line(Line::from(vec![
                marker,
                Span::styled(format!("{:<name_width$}  ", row.name), name_style),
                Span::styled(
                    format!("{:<ext_width$}  ", ext_label(row)),
                    Style::default().fg(palette.gray),
                ),
                status,
            ]));

            if let Some(err) = self.errors.get(&row.name) {
                text.push_line(Line::styled(
                    format!("      \u{2717} {err}"),
                    Style::default().fg(palette.error),
                ));
            }
        }

        if rows.iter().all(|r| r.section == Section::Installed)
            && matches!(self.index, IndexState::Loaded(_))
        {
            text.push_line(Line::raw(""));
            text.push_line(Line::styled(
                " Everything in the registry is installed.",
                Style::default().fg(palette.dim),
            ));
        }

        // Keep the selected row on screen, with a line of context when possible.
        let height = area.height as usize;
        let offset = (selected_line + 2).saturating_sub(height);
        Paragraph::new(text).scroll((offset as u16, 0)).render(area, buf);
    }

    /// The tab's key hint line.
    pub fn hint(&self) -> Line<'static> {
        let palette = crate::theme::palette();
        let key = |k: &'static str| Span::styled(k, Style::default().fg(palette.accent).bold());
        let label = |l: &'static str| Span::styled(l, Style::default().fg(palette.dim));
        Line::from(vec![
            key("j/k"),
            label(" select    "),
            key("Enter"),
            label(" install    "),
            key("r"),
            label(" refresh    "),
            key("[ / ]"),
            label(" switch tabs    "),
            key("Esc"),
            label(" close"),
        ])
        .alignment(Alignment::Center)
    }
}

fn ext_label(row: &Row) -> String {
    row.extensions.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use codemark_core::grammar::registry_source::RegistryEntry;

    fn entry(name: &str, exts: &[&str], wasm: bool) -> RegistryEntry {
        RegistryEntry {
            name: name.into(),
            extensions: exts.iter().map(|e| e.to_string()).collect(),
            profile_version: Some("2.0.0".into()),
            profile_url: format!("grammars/{name}/manifest.json"),
            ts_abi: None,
            grammar_repo: None,
            grammar_rev: None,
            description: Some(format!("{name} grammar")),
            wasm_url: wasm.then(|| format!("https://example.com/{name}.wasm")),
            wasm_sha256: wasm.then(|| "0".repeat(64)),
            overrides_builtin: false,
            stale: None,
        }
    }

    fn index(entries: Vec<RegistryEntry>) -> RegistryIndex {
        RegistryIndex { schema_version: 1, grammars: entries, stale: None }
    }

    fn installed() -> Vec<InstalledGrammar> {
        vec![
            InstalledGrammar {
                name: "dart".into(),
                extensions: vec!["dart".into()],
                builtin: true,
                profile_version: None,
            },
            InstalledGrammar {
                name: "rust".into(),
                extensions: vec!["rs".into()],
                builtin: false,
                profile_version: Some("1.0.0".into()),
            },
        ]
    }

    fn registry() -> RegistryIndex {
        index(vec![
            entry("rust", &["rs"], true),
            entry("swift", &["swift"], true),
            entry("cobol", &["cbl"], false),
        ])
    }

    #[test]
    fn rows_split_installed_from_available() {
        let rows = build_rows(&installed(), Some(&registry()));
        let summary: Vec<_> = rows.iter().map(|r| (r.name.as_str(), r.section)).collect();
        assert_eq!(
            summary,
            [
                ("dart", Section::Installed),
                ("rust", Section::Installed),
                ("cobol", Section::Available),
                ("swift", Section::Available),
            ]
        );
        assert_eq!(rows[0].note, "built-in");
        // The *installed* version (1.0.0), not the registry's current 2.0.0:
        // reading the registry would make an outdated install look current.
        assert_eq!(rows[1].note, "profile 1.0.0");
    }

    #[test]
    fn an_install_with_no_recorded_version_shows_none() {
        let local = [InstalledGrammar {
            name: "rust".into(),
            extensions: vec!["rs".into()],
            builtin: false,
            profile_version: None,
        }];
        let rows = build_rows(&local, Some(&registry()));
        assert_eq!(rows[0].note, "", "unknown, not borrowed from the registry");
    }

    #[test]
    fn an_entry_without_a_prebuilt_grammar_is_not_installable() {
        let rows = build_rows(&installed(), Some(&registry()));
        let cobol = rows.iter().find(|r| r.name == "cobol").unwrap();
        assert!(!cobol.installable);
        assert_eq!(cobol.note, "manual build");
        assert!(rows.iter().find(|r| r.name == "swift").unwrap().installable);
    }

    #[test]
    fn without_an_index_only_installed_grammars_are_listed() {
        let rows = build_rows(&installed(), None);
        assert!(rows.iter().all(|r| r.section == Section::Installed));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn an_override_is_listed_once_as_the_grammar_in_use() {
        let mut both = installed();
        both.push(InstalledGrammar {
            name: "dart".into(),
            extensions: vec!["dart".into()],
            builtin: false,
            profile_version: None,
        });
        let rows = build_rows(&both, None);
        let dart: Vec<_> = rows.iter().filter(|r| r.name == "dart").collect();
        assert_eq!(dart.len(), 1);
        assert_ne!(dart[0].note, "built-in", "the dynamic override is the one in use");
    }

    #[test]
    fn installed_matches_registry_names_case_insensitively() {
        let rows = build_rows(
            &[InstalledGrammar {
                name: "Swift".into(),
                extensions: vec!["swift".into()],
                builtin: false,
                profile_version: None,
            }],
            Some(&index(vec![entry("swift", &["swift"], true)])),
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].section, Section::Installed);
    }

    #[test]
    fn opening_the_tab_fetches_the_index_once() {
        let mut tab = LanguagesTab::with_installed(installed);
        assert_eq!(tab.on_shown(), Some(SettingsAction::FetchRegistry { refresh: false }));
        assert_eq!(tab.on_shown(), None, "in flight: don't fetch twice");
        tab.on_index_loaded(Ok(registry()));
        assert_eq!(tab.on_shown(), None, "loaded: nothing to do");
        assert_eq!(
            tab.handle_key(KeyCode::Char('r')),
            SettingsAction::FetchRegistry { refresh: true }
        );
    }

    fn select(tab: &mut LanguagesTab, name: &str) {
        tab.selected = tab.rows().iter().position(|r| r.name == name).unwrap();
    }

    #[cfg(feature = "wasm")]
    #[test]
    fn enter_installs_an_available_grammar_one_at_a_time() {
        let mut tab = LanguagesTab::with_installed(installed);
        tab.on_index_loaded(Ok(registry()));

        select(&mut tab, "swift");
        assert_eq!(tab.handle_key(KeyCode::Enter), SettingsAction::InstallGrammar("swift".into()));
        // A second install while one runs is refused, not queued.
        assert!(matches!(tab.handle_key(KeyCode::Enter), SettingsAction::Notify(_)));

        // Installed rows and manual-build rows don't start an install.
        select(&mut tab, "rust");
        assert_eq!(tab.handle_key(KeyCode::Enter), SettingsAction::Handled);
        tab.on_installed("swift", Ok(()));
        select(&mut tab, "cobol");
        assert!(
            matches!(tab.handle_key(KeyCode::Enter), SettingsAction::Notify(m) if m.contains("--from-registry"))
        );
    }

    /// A build that can't load WASM grammars refuses the install up front
    /// rather than saving a grammar it can never use.
    #[cfg(not(feature = "wasm"))]
    #[test]
    fn without_wasm_enter_explains_instead_of_installing() {
        let mut tab = LanguagesTab::with_installed(installed);
        tab.on_index_loaded(Ok(registry()));
        select(&mut tab, "swift");
        assert!(matches!(
            tab.handle_key(KeyCode::Enter),
            SettingsAction::Notify(m) if m.contains("wasm")
        ));
        assert!(tab.installing.is_none());
    }

    #[test]
    fn r_rereads_what_is_installed() {
        use std::sync::atomic::{AtomicBool, Ordering};
        static REMOVED: AtomicBool = AtomicBool::new(false);
        fn source() -> Vec<InstalledGrammar> {
            let mut all = installed();
            if REMOVED.load(Ordering::SeqCst) {
                all.retain(|g| g.name != "rust");
            }
            all
        }

        let mut tab = LanguagesTab::with_installed(source);
        tab.on_index_loaded(Ok(registry()));
        let section = |tab: &LanguagesTab| {
            tab.rows().into_iter().find(|r| r.name == "rust").map(|r| r.section)
        };
        assert_eq!(section(&tab), Some(Section::Installed));

        // Removed from the CLI while the tab stayed open: `r` shows it as
        // installable straight away, from the index already loaded.
        REMOVED.store(true, Ordering::SeqCst);
        assert_eq!(
            tab.handle_key(KeyCode::Char('r')),
            SettingsAction::FetchRegistry { refresh: true }
        );
        assert_eq!(section(&tab), Some(Section::Available));
    }

    /// A refresh keeps the loaded list, and the selection, on screen.
    #[test]
    fn refreshing_keeps_the_list_and_the_selection() {
        let mut tab = LanguagesTab::with_installed(installed);
        tab.on_index_loaded(Ok(registry()));
        select(&mut tab, "swift");
        let before = tab.rows();

        assert_eq!(
            tab.handle_key(KeyCode::Char('r')),
            SettingsAction::FetchRegistry { refresh: true }
        );
        assert_eq!(tab.rows(), before, "Available rows stay while the fetch runs");
        assert_eq!(tab.rows()[tab.selected].name, "swift");
        // One refresh at a time.
        assert_eq!(tab.handle_key(KeyCode::Char('r')), SettingsAction::Handled);

        // The registry gained an entry that sorts before swift: still on swift.
        let mut grown = registry();
        grown.grammars.push(entry("python", &["py"], true));
        tab.on_index_loaded(Ok(grown));
        assert_eq!(tab.rows()[tab.selected].name, "swift");

        // A failed refresh keeps what was there.
        tab.handle_key(KeyCode::Char('r'));
        tab.on_index_loaded(Err("offline".into()));
        assert!(tab.rows().iter().any(|r| r.name == "python"));
        assert_eq!(tab.rows()[tab.selected].name, "swift");
    }

    #[cfg(feature = "wasm")]
    #[test]
    fn a_failed_install_is_shown_and_can_be_retried() {
        let mut tab = LanguagesTab::with_installed(installed);
        tab.on_index_loaded(Ok(registry()));
        select(&mut tab, "swift");
        assert_eq!(tab.handle_key(KeyCode::Enter), SettingsAction::InstallGrammar("swift".into()));

        tab.on_installed("swift", Err("sha256 mismatch".into()));
        assert_eq!(tab.errors.get("swift").map(String::as_str), Some("sha256 mismatch"));
        assert!(tab.installing.is_none());

        // Retrying clears the error and starts again.
        assert_eq!(tab.handle_key(KeyCode::Enter), SettingsAction::InstallGrammar("swift".into()));
        assert!(!tab.errors.contains_key("swift"));
    }

    #[test]
    fn selection_stays_in_bounds() {
        let mut tab = LanguagesTab::with_installed(installed);
        for _ in 0..10 {
            tab.handle_key(KeyCode::Char('k'));
        }
        assert_eq!(tab.selected, 0);
        for _ in 0..10 {
            tab.handle_key(KeyCode::Char('j'));
        }
        assert_eq!(tab.selected, 1, "only the two installed rows before the index loads");
    }

    #[test]
    fn renders_sections_status_and_errors() {
        let mut tab = LanguagesTab::with_installed(installed);
        tab.on_index_loaded(Ok(RegistryIndex { stale: Some("offline".into()), ..registry() }));
        tab.on_installed("cobol", Err("no prebuilt grammar".into()));

        let area = Rect::new(0, 0, 80, 20);
        let mut buf = Buffer::empty(area);
        tab.render(area, &mut buf);
        let screen: String = (0..area.height)
            .map(|y| (0..area.width).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
            .collect();

        for needle in [
            "Installed",
            "Available",
            "built-in",
            ".rs",
            "swift",
            "manual build",
            "no prebuilt grammar",
            "cached list",
        ] {
            assert!(screen.contains(needle), "missing {needle:?} in:\n{screen}");
        }
    }
}
