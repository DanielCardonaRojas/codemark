use crate::browser::tabbed_panel::bookmark_health;
use crate::browser::{DetailsPaneSize, PreviewPayload, SectionConfig, StepData, TabbedPanel};
use crate::component::{Component, MarkdownPanel};
use crate::event::Event;
use codemark_core::engine::bookmark::{Bookmark, Resolution};
use codemark_core::engine::resolution as live_resolution;
use codemark_core::parser::languages::{Language as CodemarkLanguage, ParseCache};
use codemark_core::storage::db::Database;
use codemark_core::templates::{self, load_template};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Style, Stylize},
    widgets::{Block, BorderType, Widget},
};
use std::collections::HashMap;

/// Tab index for the Info tab in the steps panel.
/// The steps panel has tabs in order: Steps (0), Info (1), Query (2).
pub const INFO_TAB_INDEX: usize = 1;

/// Tab indices for the bottom details panel, which holds Details (0) and
/// Comments (1) tabs created by [`TabbedPanel::new_details_comments`]. Keeping
/// these named avoids coupling the markdown-update sites to positional literals.
const DETAILS_TAB_INDEX: usize = 0;
const COMMENTS_TAB_INDEX: usize = 1;

/// Borrowed markdown templates used to render a bookmark preview. Bundled so
/// the render path takes one grouped argument instead of three loose strings.
#[derive(Clone, Copy)]
pub(crate) struct PreviewTemplates<'a> {
    pub show: &'a str,
    pub details: &'a str,
    pub comments: &'a str,
}

/// Focus areas within the right pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightPaneFocus {
    Steps,
    Details,
}

pub struct RightPane {
    /// Steps tabbed panel (Steps/Info)
    pub steps: TabbedPanel,
    /// Collection overview panel shown live while browsing collections
    /// (before a collection is entered with Enter).
    pub overview: MarkdownPanel,
    /// When true, the overview panel replaces the steps panel in the main area.
    pub overview_active: bool,
    /// Details panel showing bookmark metadata and comments (now template-driven)
    pub details: TabbedPanel,
    /// Data for each step in the current tour
    pub steps_data: Vec<StepData>,
    /// Currently focused section
    pub focused: RightPaneFocus,
    /// Pager total pages
    pub pager_total: usize,
    /// Pager current page
    pub pager_current: usize,
    /// Last rendered area
    pub last_area: std::cell::Cell<Rect>,
    /// Last rendered details area (set during render for accurate mouse hit testing)
    last_details_area: std::cell::Cell<Rect>,
    /// Details height configuration
    pub info_config: SectionConfig,
    /// Active tour name (if a tour is loaded)
    pub active_tour_name: Option<String>,
    /// Active bookmark ID (if a single bookmark is loaded)
    pub active_bookmark_id: Option<String>,
    /// Active remote tour id (if a remote tour *overview* is shown). Remote tours
    /// aren't local collections, so they have no `active_tour_name`; tracking the
    /// id here lets `refresh_all_panels` re-render the overview after a refresh
    /// instead of falling back to the first local collection.
    pub active_remote_tour_id: Option<String>,
    /// Cached show template content to avoid repeated disk reads
    cached_show_template: String,
    /// Cached details template content to avoid repeated disk reads
    cached_details_template: String,
    /// Cached comments template content to avoid repeated disk reads
    cached_comments_template: String,
    /// Cached collection overview template content to avoid repeated disk reads
    cached_collection_overview_template: String,
    /// Flag set when step navigation changes and the caller must call update_preview
    pub needs_preview_update: bool,
    /// Cached HEAD commit hash to avoid re-running git I/O on every preview navigation
    cached_head_commit: Option<String>,
    /// Whether a bookmark preview is currently being resolved on a background
    /// task. While true the Content area shows a loading indicator instead of
    /// (possibly stale) code.
    loading: bool,
    /// Optional label (file path) shown next to the loading indicator.
    loading_label: Option<String>,
    /// Animation frame counter for the loading spinner.
    loading_tick: usize,
}

/// Render a bookmark to markdown using the given handlebars template content.
///
/// Shared by [`RightPane::generate_markdown`] and the background preview builder
/// so the two never diverge. Pure computation (no `&self`), safe to call off the
/// UI thread.
fn render_bookmark_markdown(
    db: &Database,
    bm: &Bookmark,
    resolutions: &[Resolution],
    template_content: &str,
    current_head: Option<&str>,
) -> String {
    let repo_path = db.path().parent().unwrap_or_else(|| db.path());
    render_bookmark_markdown_at(repo_path, bm, resolutions, template_content, current_head)
}

/// Render a bookmark to markdown given the repository path directly (rather than
/// a [`Database`]). The database is only ever consulted for `db.path()` here, so
/// taking the path lets this run on a background task with no DB handle — see
/// [`BrowserLayout::spawn_step_markdown_task`](crate::browser::BrowserLayout).
pub(crate) fn render_bookmark_markdown_at(
    repo_path: &std::path::Path,
    bm: &Bookmark,
    resolutions: &[Resolution],
    template_content: &str,
    current_head: Option<&str>,
) -> String {
    let context =
        templates::BookmarkTemplateContext::from_bookmark(bm, resolutions, repo_path, current_head);
    let handlebars = templates::create_handlebars_engine();

    match handlebars.render_template(template_content, &context) {
        Ok(rendered) => rendered,
        Err(e) => {
            // Fallback to simple format if template fails
            format!(
                "# Bookmark: {}\n\nError rendering template: {}",
                &bm.id[..8.min(bm.id.len())],
                e
            )
        }
    }
}

impl RightPane {
    /// Create a new right pane.
    pub fn new(db: &Database) -> Self {
        let cached_show_template = load_template(templates::SHOW_TEMPLATE);
        let cached_details_template = load_template(templates::DETAILS_TEMPLATE);
        let cached_comments_template = load_template(templates::COMMENTS_TEMPLATE);
        let cached_collection_overview_template =
            load_template(templates::COLLECTION_OVERVIEW_TEMPLATE);

        let mut pane = Self {
            steps: TabbedPanel::new_steps_info(db),
            overview: MarkdownPanel::new(),
            overview_active: false,
            details: TabbedPanel::new_details_comments(),
            steps_data: Vec::new(),
            focused: RightPaneFocus::Steps,
            pager_total: 0,
            pager_current: 0,
            last_area: std::cell::Cell::new(Rect::default()),
            last_details_area: std::cell::Cell::new(Rect::default()),
            info_config: SectionConfig::new(7, 13),
            active_tour_name: None,
            active_bookmark_id: None,
            active_remote_tour_id: None,
            needs_preview_update: false,
            cached_show_template,
            cached_details_template,
            cached_comments_template,
            cached_collection_overview_template,
            cached_head_commit: {
                let db_dir = db.path().parent().unwrap_or_else(|| db.path());
                codemark_core::git::context::detect_context(db_dir).and_then(|ctx| ctx.head_commit)
            },
            loading: false,
            loading_label: None,
            loading_tick: 0,
        };

        // Try to load the first tour automatically
        if let Ok(collections) = db.list_collections()
            && let Some((first_tour, _)) = collections.first()
        {
            let name = first_tour.name.clone();
            pane.load_tour(db, &name);
        }

        pane
    }

    /// Refresh the cached HEAD commit (call after switching databases or repos).
    pub fn refresh_head_commit(&mut self, db: &Database) {
        let db_dir = db.path().parent().unwrap_or_else(|| db.path());
        self.cached_head_commit =
            codemark_core::git::context::detect_context(db_dir).and_then(|ctx| ctx.head_commit);
    }

    /// The cached HEAD commit hash, if any (used by background preview tasks).
    pub fn head_commit(&self) -> Option<&str> {
        self.cached_head_commit.as_deref()
    }

    /// Recompute the cached health of every open collection step.
    ///
    /// `StepData.health` is captured when the collection loads, so without this
    /// the pager dots would drift from the bookmarks panel once the underlying
    /// state moves. This is the single refresh path, driven from both step
    /// navigation and live-health updates, so every staleness vector is covered:
    ///
    /// * **HEAD change** — [`refresh_head_commit`](Self::refresh_head_commit) is
    ///   called first, so a branch switch while the collection stays open (then
    ///   paging or a live-health batch) projects against the current checkout
    ///   rather than a stale cached commit.
    /// * **Bookmark change** — each bookmark is re-fetched from the database, so a
    ///   heal or sync that updates `current_resolution_id` or persisted health is
    ///   reflected instead of the load-time clone.
    ///
    /// Uses [`bookmark_health`] — the same HEAD-aware projection that seeds the
    /// value — so the pager keeps the full status nuance and matches the panel.
    /// Cheap enough for the small step count of a collection; the early return
    /// keeps it off the path where no collection is open.
    pub fn refresh_step_health(&mut self, db: &Database) {
        if self.steps_data.is_empty() {
            return;
        }
        self.refresh_head_commit(db);
        let head = self.cached_head_commit.clone();
        for step in self.steps_data.iter_mut() {
            let fresh = db.get_bookmark(&step.bookmark.id).ok().flatten();
            let bookmark = fresh.as_ref().unwrap_or(&step.bookmark);
            step.health = bookmark_health(bookmark, db, head.as_deref());
        }
    }

    /// Whether a bookmark preview is currently being resolved.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// Enter the loading state for an in-flight preview, optionally labeled with
    /// the file path so the user sees *what* is loading.
    pub fn begin_loading(&mut self, label: Option<String>) {
        self.loading = true;
        self.loading_label = label;
    }

    /// Leave the loading state without applying a preview (e.g. on failure).
    pub fn finish_loading(&mut self) {
        self.loading = false;
        self.loading_label = None;
    }

    /// Advance the loading spinner animation by one frame.
    pub fn advance_loading_spinner(&mut self) {
        self.loading_tick = self.loading_tick.wrapping_add(1);
    }

    /// Re-apply the active syntax theme to the code preview panes, re-highlighting
    /// their current content. Called after a runtime theme change so existing
    /// previews don't keep their old colors.
    pub fn reapply_theme(&mut self) {
        self.steps.reapply_preview_theme();
    }

    /// Fully re-render the preview for the current step: the code pane *and* the
    /// Info/Details/Comments markdown, synchronously.
    ///
    /// Used by the load paths (entering a collection/tour, single-bookmark
    /// preview), where one synchronous render is fine. The per-keypress paging
    /// path does **not** use this — it splits into [`update_step_code`] (cheap,
    /// inline) plus a background markdown render (see
    /// [`BrowserLayout::spawn_step_markdown_task`](crate::browser::BrowserLayout))
    /// so paging never blocks on the expensive markdown.
    ///
    /// This deliberately does **not** call [`refresh_step_health`], which spawns
    /// a `git rev-parse` subprocess and runs per-step git ancestry queries; step
    /// health is kept current by `LiveHealthBatch`/`HealComplete`/`SyncComplete`
    /// and refreshed on `FocusGained` (the only time HEAD moves while a
    /// collection stays open).
    pub fn update_preview(&mut self, db: &Database) {
        self.update_step_code();
        self.update_step_markdown(db);
    }

    /// Update only the code pane (and the cheap Query tab) for the current step.
    ///
    /// This is the visible part the user watches while paging, and it is cheap —
    /// a file read plus a syntax highlight that is cached per step (see
    /// [`CodePreview::set_code_keyed`](crate::component::CodePreview::set_code_keyed))
    /// — so it runs inline on every pager move for instant feedback. The
    /// expensive Info/Details/Comments markdown is rendered off-thread instead.
    pub fn update_step_code(&mut self) {
        let Some(step) = self.steps_data.get(self.pager_current) else { return };

        let code = std::fs::read_to_string(&step.file_path)
            .unwrap_or_else(|_| format!("Error: Could not load file {}", step.file_path));
        let ext = std::path::Path::new(&step.file_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("txt")
            .to_string();
        // Use the bookmark's relative file path (or the resolution's override).
        let relative_path = step
            .resolution
            .as_ref()
            .and_then(|r| r.file_path.clone())
            .unwrap_or_else(|| step.bookmark.file_path.clone());
        let resolved = step.resolved;
        let line_number = step.line_number;
        let line_end = step.line_end;
        let query = step.bookmark.query.clone();

        if let Some(preview) = self.steps.get_step_preview_mut() {
            // Keyed set reuses a cached highlight when this step was visited
            // before, and sets code+extension together to avoid a double
            // highlight when the language changes between steps.
            preview.set_code_keyed(code, ext);
            tracing::debug!(target: "codemark::ui", %relative_path, "Setting preview file header");
            preview.set_file_header(Some(relative_path));
            if resolved {
                preview.jump_to_range(line_number, line_end);
            } else {
                // Query no longer resolves: show the file without a stale
                // range highlight/gutter.
                preview.clear_selection();
            }
        }

        // The Query tab is just the raw query text (no rendering), so set it here.
        if let Some(query_preview) = self.steps.get_query_preview_mut() {
            query_preview.set_code(query);
            query_preview.set_extension("scm".to_string());
        }
    }

    /// Render and apply the Info/Details/Comments markdown for the current step
    /// synchronously. Split out of [`update_preview`] so the paging path can run
    /// the same work off-thread; the load paths call it inline via
    /// [`update_preview`].
    fn update_step_markdown(&mut self, db: &Database) {
        let Some(step) = self.steps_data.get(self.pager_current) else { return };
        let head_ref = self.cached_head_commit.as_deref();

        let info_markdown = self.generate_markdown(
            db,
            &step.bookmark,
            &step.resolutions,
            templates::SHOW_TEMPLATE,
            head_ref,
        );
        let details_markdown = self.generate_markdown(
            db,
            &step.bookmark,
            &step.resolutions,
            templates::DETAILS_TEMPLATE,
            head_ref,
        );
        let comments_markdown = self.generate_markdown(
            db,
            &step.bookmark,
            &step.resolutions,
            templates::COMMENTS_TEMPLATE,
            head_ref,
        );
        self.apply_step_markdown(crate::browser::StepPreviewMarkdown {
            info_markdown,
            details_markdown,
            comments_markdown,
        });
    }

    /// Apply pre-rendered step markdown to the Info tab and the bottom
    /// Details/Comments panes. Pure UI-thread assignment (no rendering), so it
    /// never blocks — this is the apply half of the background markdown render.
    pub fn apply_step_markdown(&mut self, markdown: crate::browser::StepPreviewMarkdown) {
        let crate::browser::StepPreviewMarkdown {
            info_markdown,
            details_markdown,
            comments_markdown,
        } = markdown;
        if let Some(md_panel) = self.steps.get_markdown_mut() {
            md_panel.set_markdown(info_markdown);
        }
        self.set_details_and_comments(details_markdown, comments_markdown);
    }

    /// The bookmark id of the step currently shown, if a collection/tour step is
    /// loaded. Used to drop a background step-markdown result once the user has
    /// paged on to a different step.
    pub fn current_step_bookmark_id(&self) -> Option<&str> {
        self.steps_data.get(self.pager_current).map(|s| s.bookmark.id.as_str())
    }

    /// Snapshot the current step's bookmark and resolutions for an off-thread
    /// markdown render. Returns `None` when no step is loaded.
    pub fn current_step_render_input(&self) -> Option<(Bookmark, Vec<Resolution>)> {
        self.steps_data.get(self.pager_current).map(|s| (s.bookmark.clone(), s.resolutions.clone()))
    }

    /// The cached Info/Details/Comments template strings, cloned so a background
    /// task can render markdown without touching the (non-`Send`) `RightPane`.
    pub fn markdown_templates(&self) -> (String, String, String) {
        (
            self.cached_show_template.clone(),
            self.cached_details_template.clone(),
            self.cached_comments_template.clone(),
        )
    }

    // @lat: [[tui-line-range-selection#Load bookmark with range]]
    /// Load a single bookmark for previewing.
    pub fn load_bookmark(&mut self, db: &Database, bookmark_id: &str) {
        if let Ok(Some(bm)) = db.get_bookmark(bookmark_id) {
            let mut line_number = 0;
            let mut line_end = None;
            let mut file_path = bm.file_path.clone();

            // Get the best resolution for preview (from nearest ancestor commit)
            let resolution = db.get_preview_resolution(&bm.id).ok().flatten();

            // Get all resolutions for showing full history
            let resolutions = db.list_resolutions(&bm.id, 100).unwrap_or_default();

            // Extract line_range and file_path from the resolution
            if let Some(ref res) = resolution {
                if let Some(fp) = res.file_path.as_ref() {
                    file_path = fp.clone();
                }
                if let Some(lr) = res.line_range.as_ref() {
                    let parts: Vec<&str> = lr.split('-').collect();
                    if let (Some(start), Some(end)) = (
                        parts.first().and_then(|s| s.parse::<usize>().ok()),
                        parts.get(1).and_then(|s| s.parse::<usize>().ok()),
                    ) {
                        line_number = start.saturating_sub(1);
                        line_end = Some(end.saturating_sub(1));
                    } else if let Some(start) = parts.first().and_then(|s| s.parse::<usize>().ok())
                    {
                        line_number = start.saturating_sub(1);
                    }
                }
            }

            if let Ok(abs_path) =
                codemark_core::git::context::resolve_bookmark_file_path(&file_path, db.path())
            {
                let health = bookmark_health(&bm, db, self.cached_head_commit.as_deref());
                self.steps_data = vec![StepData {
                    file_path: abs_path.to_string_lossy().to_string(),
                    line_number,
                    line_end,
                    bookmark: bm,
                    resolution,
                    resolutions,
                    health,
                    // Persisted fallback: the query didn't resolve live, so the
                    // range is stale and must not be highlighted.
                    resolved: false,
                }];
                self.pager_total = 1;
                self.pager_current = 0;
                self.active_bookmark_id = Some(bookmark_id.to_string());
                self.active_tour_name = None;
                self.active_remote_tour_id = None;
                self.overview_active = false;
                self.update_preview(db);
            }
        } else {
            // Bookmark not found - clear stale preview state
            self.clear_preview_state(db);
        }
    }

    /// Load a single bookmark for previewing using live (on-the-fly) resolution.
    ///
    /// Runs `resolve_transient()` synchronously via `block_on` to get the current
    /// location of the bookmarked code directly from disk + tree-sitter, without
    /// reading persisted resolutions from the database. Falls back to the
    /// persisted path (`load_bookmark()`) on error.
    pub fn load_bookmark_live(
        &mut self,
        db: &Database,
        bookmark_id: &str,
        session_cache: &mut HashMap<CodemarkLanguage, ParseCache>,
    ) {
        match Self::build_bookmark_preview(
            db,
            bookmark_id,
            session_cache,
            PreviewTemplates {
                show: &self.cached_show_template,
                details: &self.cached_details_template,
                comments: &self.cached_comments_template,
            },
            self.cached_head_commit.as_deref(),
            // Synchronous path runs on the UI runtime worker thread.
            true,
        ) {
            Some(payload) => self.apply_preview(*payload),
            None => {
                // Live resolution failed (or bookmark missing); fall back to the
                // persisted path, which also clears state when not found.
                tracing::warn!(
                    target: "codemark::ui",
                    bookmark_id = %bookmark_id,
                    "Live resolution failed, falling back to persisted path"
                );
                self.load_bookmark(db, bookmark_id);
            }
        }
    }

    /// Build a fully-computed bookmark preview using live (on-the-fly) resolution.
    ///
    /// Does all the expensive work — live resolution, file read, markdown
    /// rendering — and returns a [`PreviewPayload`] ready to apply with
    /// [`apply_preview`](Self::apply_preview). Returns `None` when the bookmark
    /// is missing or cannot be resolved live (caller falls back to the persisted
    /// path). This is a free-standing computation (no `&self`) so it can run on a
    /// background task as well as synchronously.
    pub(crate) fn build_bookmark_preview(
        db: &Database,
        bookmark_id: &str,
        cache: &mut HashMap<CodemarkLanguage, ParseCache>,
        templates: PreviewTemplates<'_>,
        head: Option<&str>,
        on_runtime_worker: bool,
    ) -> Option<Box<PreviewPayload>> {
        let bm = db.get_bookmark(bookmark_id).ok().flatten()?;
        let (abs_path, start_line, end_line, code) =
            Self::resolve_bookmark_live(&bm, db, cache, on_runtime_worker).ok()?;

        let resolutions = db.list_resolutions(&bm.id, 100).unwrap_or_default();
        let extension = std::path::Path::new(&abs_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("txt")
            .to_string();
        // Live resolution carries no persisted resolution, so the header uses
        // the bookmark's own relative path (matches `update_preview`).
        let relative_path = bm.file_path.clone();
        let info_markdown = render_bookmark_markdown(db, &bm, &resolutions, templates.show, head);
        let details_markdown =
            render_bookmark_markdown(db, &bm, &resolutions, templates.details, head);
        let comments_markdown =
            render_bookmark_markdown(db, &bm, &resolutions, templates.comments, head);
        let query = bm.query.clone();
        let health = bookmark_health(&bm, db, head);

        let step = StepData {
            file_path: abs_path,
            line_number: start_line,
            line_end: Some(end_line),
            bookmark: bm,
            resolution: None,
            resolutions,
            health,
            // Live resolution succeeded, so the range reflects the current
            // location and is safe to highlight.
            resolved: true,
        };

        Some(Box::new(PreviewPayload {
            bookmark_id: bookmark_id.to_string(),
            step,
            code,
            extension,
            relative_path,
            info_markdown,
            details_markdown,
            comments_markdown,
            query,
        }))
    }

    /// Apply a pre-computed preview to the panels. Pure UI-thread work (no I/O):
    /// just assigns the already-rendered content, so it never blocks the loop.
    pub fn apply_preview(&mut self, payload: PreviewPayload) {
        self.finish_loading();

        let PreviewPayload {
            bookmark_id,
            step,
            code,
            extension,
            relative_path,
            info_markdown,
            details_markdown,
            comments_markdown,
            query,
        } = payload;

        if let Some(preview) = self.steps.get_step_preview_mut() {
            preview.set_code(code);
            preview.set_extension(extension);
            preview.set_file_header(Some(relative_path));
            if step.resolved {
                preview.jump_to_range(step.line_number, step.line_end);
            } else {
                preview.clear_selection();
            }
        }
        if let Some(md_panel) = self.steps.get_markdown_mut() {
            md_panel.set_markdown(info_markdown);
        }
        if let Some(query_preview) = self.steps.get_query_preview_mut() {
            query_preview.set_code(query);
            query_preview.set_extension("scm".to_string());
        }
        self.set_details_and_comments(details_markdown, comments_markdown);

        self.steps_data = vec![step];
        self.pager_total = 1;
        self.pager_current = 0;
        self.active_bookmark_id = Some(bookmark_id);
        self.active_tour_name = None;
        self.active_remote_tour_id = None;
        self.overview_active = false;
    }

    /// Load a tour using live resolution for each step.
    ///
    /// Same pattern as `load_tour()` but uses `resolve_transient()` for each
    /// bookmark in the collection to get current-disk locations.
    pub fn load_tour_live(
        &mut self,
        db: &Database,
        tour_name: &str,
        session_cache: &mut HashMap<CodemarkLanguage, ParseCache>,
    ) {
        let Some(collection) = db.get_collection_by_name(tour_name).ok().flatten() else {
            self.clear_preview_state(db);
            return;
        };

        let Ok(bookmarks) = db.list_bookmarks_in_collection(&collection.id) else {
            self.clear_preview_state(db);
            return;
        };

        let mut new_steps = Vec::new();
        for bm in bookmarks {
            let resolutions = db.list_resolutions(&bm.id, 100).unwrap_or_default();

            let health = bookmark_health(&bm, db, self.cached_head_commit.as_deref());
            match Self::resolve_bookmark_live(&bm, db, session_cache, true) {
                Ok((abs_path, start_line, end_line, _source)) => {
                    new_steps.push(StepData {
                        file_path: abs_path,
                        line_number: start_line,
                        line_end: Some(end_line),
                        bookmark: bm,
                        resolution: None,
                        resolutions,
                        health,
                        // Live resolution succeeded for this step.
                        resolved: true,
                    });
                }
                Err(_) => {
                    // Fallback to persisted resolution for this step
                    let mut line_number = 0;
                    let mut line_end = None;
                    let mut file_path = bm.file_path.clone();
                    let resolution = db.get_preview_resolution(&bm.id).ok().flatten();

                    if let Some(ref res) = resolution {
                        if let Some(fp) = res.file_path.as_ref() {
                            file_path = fp.clone();
                        }
                        if let Some(lr) = res.line_range.as_ref() {
                            let parts: Vec<&str> = lr.split('-').collect();
                            if let (Some(start), Some(end)) = (
                                parts.first().and_then(|s| s.parse::<usize>().ok()),
                                parts.get(1).and_then(|s| s.parse::<usize>().ok()),
                            ) {
                                line_number = start.saturating_sub(1);
                                line_end = Some(end.saturating_sub(1));
                            } else if let Some(start) =
                                parts.first().and_then(|s| s.parse::<usize>().ok())
                            {
                                line_number = start.saturating_sub(1);
                            }
                        }
                    }

                    if let Ok(abs_path) = codemark_core::git::context::resolve_bookmark_file_path(
                        &file_path,
                        db.path(),
                    ) {
                        new_steps.push(StepData {
                            file_path: abs_path.to_string_lossy().to_string(),
                            line_number,
                            line_end,
                            bookmark: bm,
                            resolution,
                            resolutions,
                            health,
                            // Live resolution failed; the persisted range is
                            // stale, so don't highlight it.
                            resolved: false,
                        });
                    }
                }
            }
        }

        if !new_steps.is_empty() {
            self.steps_data = new_steps;
            self.pager_total = self.steps_data.len();
            self.pager_current = 0;
            self.active_tour_name = Some(tour_name.to_string());
            self.active_bookmark_id = None;
            self.active_remote_tour_id = None;
            self.overview_active = false;
            self.update_preview(db);
        } else {
            self.clear_preview_state(db);
        }
    }

    /// Resolve a bookmark on-the-fly using tree-sitter, returning
    /// `(abs_path, start_line, end_line, source)`. Line numbers are 0-indexed
    /// (from tree-sitter `Point.row`); `source` is the file's contents, returned
    /// from the parse cache so callers don't read the file a second time.
    ///
    /// The session [`ParseCache`] is reused across selections and invalidates by
    /// mtime, so scrolling through bookmarks in the same (unchanged) file is a
    /// cache hit — no disk read, no re-parse. External edits are still picked up
    /// because the mtime changes.
    ///
    /// `on_runtime_worker` selects how the async resolution future is driven to
    /// completion. When called from a Tokio runtime worker thread (the UI event
    /// loop) it must use `block_in_place` so the worker isn't blocked; when
    /// called from a `spawn_blocking` thread (the background preview task)
    /// `block_in_place` would panic, so we drive the future with a plain
    /// `block_on` instead.
    fn resolve_bookmark_live(
        bm: &Bookmark,
        db: &Database,
        session_cache: &mut HashMap<CodemarkLanguage, ParseCache>,
        on_runtime_worker: bool,
    ) -> std::result::Result<(String, usize, usize, String), codemark_core::error::Error> {
        use std::str::FromStr;

        let language = CodemarkLanguage::from_str(&bm.language).map_err(|e| {
            codemark_core::error::Error::Input(format!(
                "unsupported language {}: {}",
                bm.language, e
            ))
        })?;

        // Get or create a ParseCache for this language
        let cache = match session_cache.entry(language) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let pc = ParseCache::new(language).map_err(|err| {
                    codemark_core::error::Error::TreeSitter(format!(
                        "failed to create ParseCache for {}: {}",
                        bm.language, err
                    ))
                })?;
                e.insert(pc)
            }
        };

        let provider = codemark_core::vfs::LocalFileProvider;
        let handle = tokio::runtime::Handle::current();

        // Resolve, then pull the source straight out of the cache (a hit, since
        // resolution just parsed this file) to avoid a second disk read.
        let resolve = async {
            let result =
                live_resolution::resolve_transient(bm, cache, language, db.path(), &provider)
                    .await?;

            // Treat Failed resolutions as errors so callers fall back to persisted snapshots
            if result.live_status() == live_resolution::LiveUIStatus::Broken {
                return Err(codemark_core::error::Error::Resolution(
                    "bookmark code not found in current file".into(),
                ));
            }

            let abs_path = codemark_core::git::context::resolve_bookmark_file_path(
                &result.file_path,
                db.path(),
            )?;
            let (_tree, source) = cache.get_or_parse(&abs_path, &provider).await?;

            Ok::<_, codemark_core::error::Error>((
                abs_path.to_string_lossy().to_string(),
                result.start_line,
                result.end_line,
                source.clone(),
            ))
        };

        if on_runtime_worker {
            // On a runtime worker thread (UI loop): yield the worker while blocking.
            tokio::task::block_in_place(|| handle.block_on(resolve))
        } else {
            // On a spawn_blocking thread: block_in_place would panic, so block directly.
            handle.block_on(resolve)
        }
    }

    /// Clear the preview state (used when a bookmark cannot be loaded).
    ///
    /// Also clears rendered panels so stale content from a previous bookmark
    /// does not remain visible.
    pub fn clear_preview_state(&mut self, db: &Database) {
        self.steps_data.clear();
        self.pager_total = 0;
        self.pager_current = 0;
        self.active_bookmark_id = None;
        self.active_tour_name = None;
        self.active_remote_tour_id = None;
        self.overview_active = false;
        self.overview.set_markdown(String::new());

        // Clear the rendered preview panels so old content doesn't linger
        if let Some(preview) = self.steps.get_step_preview_mut() {
            preview.set_code(String::new());
            preview.set_file_header(None);
        }
        if let Some(md_panel) = self.steps.get_markdown_mut() {
            md_panel.set_markdown(String::new());
        }
        if let Some(query_preview) = self.steps.get_query_preview_mut() {
            query_preview.set_code(String::new());
        }
        self.set_details_and_comments(String::new(), String::new());

        // Still call update_preview for any additional side effects
        self.update_preview(db);
    }

    /// Load a live collection overview for the given collection ID.
    ///
    /// Renders collection metadata (description, health, tags, links, and the
    /// ordered list of steps) into the overview panel, which replaces the steps
    /// panel in the main area while browsing collections. This is shown *before*
    /// a collection is entered with Enter; entering loads the per-step code
    /// previews via [`load_tour_live`].
    pub fn load_collection_overview(&mut self, db: &Database, collection_id: &str) {
        let Some(collection) = db.get_collection_by_id(collection_id).ok().flatten() else {
            self.clear_preview_state(db);
            return;
        };

        let bookmarks = db.list_bookmarks_in_collection(&collection.id).unwrap_or_default();
        let tags = db
            .list_tags_for_collection(&collection.id)
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.tag)
            .collect::<Vec<_>>();
        let links = db.list_links_for_collection(&collection.id).unwrap_or_default();

        let markdown = templates::render_collection_overview_with_template(
            &self.cached_collection_overview_template,
            &collection,
            &bookmarks,
            tags,
            &links,
        )
        .unwrap_or_else(|e| format!("# {}\n\nError rendering overview: {}", collection.name, e));

        self.overview.set_markdown(markdown);
        self.overview_active = true;

        // Clear the Details pane so stale annotations from a previously viewed
        // bookmark don't linger beneath the overview.
        self.set_details_and_comments(String::new(), String::new());

        // No per-step code preview while showing the overview.
        self.steps_data.clear();
        self.pager_total = 0;
        self.pager_current = 0;
        self.active_tour_name = Some(collection.name.clone());
        self.active_bookmark_id = None;
        self.active_remote_tour_id = None;
    }

    /// Load an overview for a remote (not-yet-pulled) tour into the overview
    /// panel. Mirrors [`load_collection_overview`](Self::load_collection_overview)
    /// but renders server-supplied metadata (title, author, repo, updated) so the
    /// user can preview a tour before pulling it. There is no per-step code
    /// preview because the tour's bookmarks aren't available locally yet.
    pub fn load_tour_overview(&mut self, tour: &codemark_core::sync::RemoteTourSummary) {
        let markdown = templates::render_remote_tour_overview(tour);

        self.overview.set_markdown(markdown);
        self.overview_active = true;

        // Clear the Details pane so stale annotations from a previously viewed
        // bookmark don't linger beneath the overview.
        self.set_details_and_comments(String::new(), String::new());

        // No per-step code preview while showing a remote tour overview.
        self.steps_data.clear();
        self.pager_total = 0;
        self.pager_current = 0;
        self.active_tour_name = None;
        self.active_bookmark_id = None;
        // Remember which remote tour is shown so a later refresh can restore this
        // overview instead of falling back to the first local collection.
        self.active_remote_tour_id = Some(tour.tour_id.clone());
    }

    /// Load a tour and its steps from the database.
    pub fn load_tour(&mut self, db: &Database, tour_name: &str) {
        let Some(collection) = db.get_collection_by_name(tour_name).ok().flatten() else {
            // Collection not found - clear stale preview state
            self.clear_preview_state(db);
            return;
        };

        let Ok(bookmarks) = db.list_bookmarks_in_collection(&collection.id) else {
            // Failed to load bookmarks - clear stale preview state
            self.clear_preview_state(db);
            return;
        };

        {
            let mut new_steps = Vec::new();
            for bm in bookmarks {
                let mut line_number = 0;
                let mut line_end = None;
                let mut file_path = bm.file_path.clone();

                // Get the best resolution for preview (from nearest ancestor commit)
                let resolution = db.get_preview_resolution(&bm.id).ok().flatten();

                // Get all resolutions for showing full history
                let resolutions = db.list_resolutions(&bm.id, 100).unwrap_or_default();

                // Extract line_range and file_path from the resolution
                if let Some(ref res) = resolution {
                    if let Some(fp) = res.file_path.as_ref() {
                        file_path = fp.clone();
                    }
                    if let Some(lr) = res.line_range.as_ref() {
                        let parts: Vec<&str> = lr.split('-').collect();
                        if let (Some(start), Some(end)) = (
                            parts.first().and_then(|s| s.parse::<usize>().ok()),
                            parts.get(1).and_then(|s| s.parse::<usize>().ok()),
                        ) {
                            line_number = start.saturating_sub(1);
                            line_end = Some(end.saturating_sub(1));
                        } else if let Some(start) =
                            parts.first().and_then(|s| s.parse::<usize>().ok())
                        {
                            line_number = start.saturating_sub(1);
                        }
                    }
                }

                // Resolve absolute path
                if let Ok(abs_path) =
                    codemark_core::git::context::resolve_bookmark_file_path(&file_path, db.path())
                {
                    let health = bookmark_health(&bm, db, self.cached_head_commit.as_deref());
                    new_steps.push(StepData {
                        file_path: abs_path.to_string_lossy().to_string(),
                        line_number,
                        line_end,
                        bookmark: bm,
                        resolution,
                        resolutions,
                        health,
                        // Persisted (non-live) load: keep highlighting the stored
                        // range as before.
                        resolved: true,
                    });
                }
            }

            if !new_steps.is_empty() {
                self.steps_data = new_steps;
                self.pager_total = self.steps_data.len();
                self.pager_current = 0;
                self.active_tour_name = Some(tour_name.to_string());
                self.active_bookmark_id = None;
                self.active_remote_tour_id = None;
                self.overview_active = false;
                self.update_preview(db);
            } else {
                // Clear the right-pane state when no steps are available
                self.clear_preview_state(db);
            }
        }
    }

    /// Generate markdown for a bookmark using a specific template.
    pub fn generate_markdown(
        &self,
        db: &Database,
        bm: &Bookmark,
        resolutions: &[Resolution],
        template: &str,
        current_head: Option<&str>,
    ) -> String {
        // Select the appropriate cached template to avoid repeated disk reads
        let template_content = match template {
            templates::SHOW_TEMPLATE => &self.cached_show_template,
            templates::DETAILS_TEMPLATE => &self.cached_details_template,
            templates::COMMENTS_TEMPLATE => &self.cached_comments_template,
            _ => {
                // Fallback for unknown templates (shouldn't happen in normal use)
                return format!(
                    "# Bookmark: {}\n\nError: Unknown template {}",
                    &bm.id[..8.min(bm.id.len())],
                    template
                );
            }
        };

        render_bookmark_markdown(db, bm, resolutions, template_content, current_head)
    }

    /// Render the right pane.
    ///
    /// # Arguments
    /// * `area` - The area to render in
    /// * `buf` - The buffer to render to
    /// * `hide_details` - If true, hide the details pane (preview + pagination
    ///   only). The pagination widget stays visible regardless.
    /// * `details_size` - Current size mode for the details pane
    pub fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        hide_details: bool,
        details_size: DetailsPaneSize,
    ) {
        self.last_area.set(area);

        // Collections have no notes or comments, so the details pane (Details /
        // Comments tabs) is meaningless while previewing a collection overview.
        // Hide it until the collection is entered and per-bookmark previews load.
        let hide_details = hide_details || self.overview_active;

        if !hide_details && details_size.is_expanded() {
            // Details takes the full right-pane area (steps/pager hidden)
            self.render_details_block(area, buf);
            return;
        }

        let info_height = if self.focused == RightPaneFocus::Details {
            self.info_config.max
        } else {
            self.info_config.min
        };

        // If only one step, hide the pager. In preview-only mode the details
        // pane collapses to zero height but the pager is kept.
        let pager_height = if self.pager_total > 1 { 1 } else { 0 };
        let details_height = if hide_details { 0 } else { info_height };
        let constraints = vec![
            Constraint::Min(0),
            Constraint::Length(pager_height),
            Constraint::Length(details_height),
        ];

        // Split vertically: steps (flex), pager (1 row or 0), details (dynamic height)
        let chunks =
            Layout::default().direction(Direction::Vertical).constraints(constraints).split(area);

        // Render the collection overview (live collection browsing) or the
        // steps tabbed panel (single bookmark / entered tour).
        if self.overview_active {
            self.render_overview_block(chunks[0], buf);
        } else {
            self.render_steps_or_loading(chunks[0], buf);
        }

        // Render pager if needed
        if self.pager_total > 1 {
            use crate::component::Pager;
            let health = self.steps_data.iter().map(|step| step.health).collect();
            let pager = Pager::new(self.pager_total, self.pager_current).with_health(health);
            pager.render(chunks[1], buf);
        }

        if !hide_details {
            self.render_details_block(chunks[2], buf);
        }
    }

    /// Borrow the markdown panel backing one of the bottom details tabs
    /// (`DETAILS_TAB_INDEX` or `COMMENTS_TAB_INDEX`). Returns `None` if the tab
    /// isn't a markdown panel, so callers can update content without depending
    /// on the positional layout of `new_details_comments`.
    fn details_markdown_mut(&mut self, tab_index: usize) -> Option<&mut MarkdownPanel> {
        match self.details.panels.get_mut(tab_index) {
            Some(crate::browser::TabContent::Markdown(md)) => Some(md),
            _ => None,
        }
    }

    /// Set the markdown for both bottom details tabs in one call.
    fn set_details_and_comments(&mut self, details_markdown: String, comments_markdown: String) {
        if let Some(md) = self.details_markdown_mut(DETAILS_TAB_INDEX) {
            md.set_markdown(details_markdown);
        }
        if let Some(md) = self.details_markdown_mut(COMMENTS_TAB_INDEX) {
            md.set_markdown(comments_markdown);
        }
    }

    /// Render the details block with border, title offset, and content.
    fn render_details_block(&self, area: Rect, buf: &mut Buffer) {
        self.last_details_area.set(area);
        self.details.render(area, buf);
    }

    /// Render either the steps panel or, while a preview is resolving in the
    /// background, a loading indicator in its place.
    fn render_steps_or_loading(&self, area: Rect, buf: &mut Buffer) {
        if self.loading {
            self.render_loading_block(area, buf);
        } else {
            self.steps.render(area, buf);
        }
    }

    /// Render a bordered "Content" block with a centered animated spinner while a
    /// bookmark preview resolves on a background task.
    fn render_loading_block(&self, area: Rect, buf: &mut Buffer) {
        const SPINNER_FRAMES: &[&str] = &[
            "\u{28cb}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
            "\u{2827}", "\u{2807}", "\u{280f}",
        ];
        let frame = SPINNER_FRAMES[self.loading_tick % SPINNER_FRAMES.len()];

        let border_style = if self.focused == RightPaneFocus::Steps {
            Style::default().fg(crate::theme::palette().accent)
        } else {
            Style::default().fg(crate::theme::palette().dim)
        };
        let mut title = ratatui::text::Line::from(vec![
            ratatui::text::Span::raw("  "),
            ratatui::text::Span::styled("Content", Style::default().bold()),
        ]);
        // Reserve room on the right so the `[4]` badge can't overwrite the title
        // on a narrow pane (mirrors `TabbedPanel::render`).
        let reserved = crate::browser::tabs::pane_number_badge_reserved_width(4);
        let max_title_width = (area.width as usize).saturating_sub(reserved as usize + 1);
        title = crate::browser::tabs::truncate_line_to_width(title, max_title_width);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(title)
            .border_style(border_style);
        let inner = block.inner(area);
        block.render(area, buf);

        // Extend the top border line after the ╭ character (matching tabbed panels)
        for i in 1..=2u16 {
            let x = area.left() + i;
            let y = area.top();
            if x < area.right()
                && let Some(cell) = buf.cell_mut((x, y))
            {
                cell.set_char('─');
                cell.set_style(border_style);
            }
        }

        // Match the steps panel's `[4]` badge while its preview is loading.
        crate::browser::tabs::render_pane_number_badge(area, buf, 4, border_style);

        let message = match &self.loading_label {
            Some(label) => format!("{frame}  Loading {label}…"),
            None => format!("{frame}  Loading preview…"),
        };
        let line = ratatui::text::Line::from(ratatui::text::Span::styled(
            message,
            Style::default().fg(crate::theme::palette().dim),
        ));
        let paragraph =
            ratatui::widgets::Paragraph::new(line).alignment(ratatui::layout::Alignment::Center);
        // Vertically center within the inner area.
        if inner.height > 0 {
            let y = inner.y + inner.height / 2;
            let row = Rect { x: inner.x, y, width: inner.width, height: 1 };
            paragraph.render(row, buf);
        }
    }

    /// Render the collection overview block with border, title offset, and content.
    ///
    /// Mirrors the steps tabbed panel's framing so the live collection overview
    /// occupies the same main area while browsing collections.
    fn render_overview_block(&self, area: Rect, buf: &mut Buffer) {
        let border_style = if self.focused == RightPaneFocus::Steps {
            Style::default().fg(crate::theme::palette().accent)
        } else {
            Style::default().fg(crate::theme::palette().dim)
        };

        let mut title = ratatui::text::Line::from(vec![
            ratatui::text::Span::raw("  "),
            ratatui::text::Span::styled("Overview", Style::default().bold()),
        ]);
        // Reserve room on the right so the `[4]` badge can't overwrite the title
        // on a narrow pane (mirrors `TabbedPanel::render`).
        let reserved = crate::browser::tabs::pane_number_badge_reserved_width(4);
        let max_title_width = (area.width as usize).saturating_sub(reserved as usize + 1);
        title = crate::browser::tabs::truncate_line_to_width(title, max_title_width);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .title(title)
            .border_style(border_style);

        let inner = block.inner(area);
        block.render(area, buf);

        // Extend the top border line after the ╭ character (matching tabbed panels)
        for i in 1..=2u16 {
            let x = area.left() + i;
            let y = area.top();
            if x < area.right()
                && let Some(cell) = buf.cell_mut((x, y))
            {
                cell.set_char('─');
                cell.set_style(border_style);
            }
        }

        // Match the steps panel's `[4]` badge while browsing a collection.
        crate::browser::tabs::render_pane_number_badge(area, buf, 4, border_style);

        self.overview.render(inner, buf);
    }

    /// Handle an event.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        // Handle mouse clicks for internal focus switching
        if let Event::Mouse(mouse) = event
            && let ratatui::crossterm::event::MouseEventKind::Down(
                ratatui::crossterm::event::MouseButton::Left,
            ) = mouse.kind
        {
            let col = mouse.column;
            let row = mouse.row;

            let steps_area = self.steps.last_area();
            if col >= steps_area.x
                && col < steps_area.x + steps_area.width
                && row >= steps_area.y
                && row < steps_area.y + steps_area.height
            {
                self.focus_steps();
            } else {
                let details_area = self.last_details_area.get();
                if col >= details_area.x
                    && col < details_area.x + details_area.width
                    && row >= details_area.y
                    && row < details_area.y + details_area.height
                {
                    self.focus_details();
                }
            }
        }

        // When the overview is shown, it takes the place of the steps panel in
        // the main area, so route "Steps"-bound events to the overview instead.
        let handled = match event {
            Event::Mouse(_) => {
                // For mouse events, check both to allow scrolling any hovered pane
                let main = if self.overview_active {
                    self.overview.handle_event(event)
                } else {
                    self.steps.handle_event(event)
                };
                main || self.details.handle_event(event)
            }
            _ => {
                // For keyboard events, follow focus
                match self.focused {
                    RightPaneFocus::Steps if self.overview_active => {
                        self.overview.handle_event(event)
                    }
                    RightPaneFocus::Steps => self.steps.handle_event(event),
                    RightPaneFocus::Details => self.details.handle_event(event),
                }
            }
        };

        if handled {
            return true;
        }

        // Handle navigation within right pane if not handled by components
        if let Event::Key(key) = event {
            match key.code {
                // Left/right navigation works for both Steps and Details focus
                ratatui::crossterm::event::KeyCode::Left
                | ratatui::crossterm::event::KeyCode::Char('h') => {
                    if self.pager_current > 0 {
                        self.pager_current = self.pager_current.saturating_sub(1);
                        self.needs_preview_update = true;
                    }
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Right
                | ratatui::crossterm::event::KeyCode::Char('l') => {
                    if self.pager_current + 1 < self.pager_total {
                        self.pager_current += 1;
                        self.needs_preview_update = true;
                    }
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Down
                    if self.focused == RightPaneFocus::Steps =>
                {
                    self.focus_details();
                    return true;
                }
                ratatui::crossterm::event::KeyCode::Up
                    if self.focused == RightPaneFocus::Details =>
                {
                    self.focus_steps();
                    return true;
                }
                _ => {}
            }
        }

        false
    }

    /// Set focus state.
    pub fn set_focus(&mut self, focused: bool) {
        match self.focused {
            RightPaneFocus::Steps if self.overview_active => self.overview.set_focus(focused),
            RightPaneFocus::Steps => self.steps.set_focus(focused),
            RightPaneFocus::Details => self.details.set_focus(focused),
        }
    }

    /// Focus the steps section (or the overview when it occupies the main area).
    pub fn focus_steps(&mut self) {
        self.focused = RightPaneFocus::Steps;
        // Only the panel actually rendered in the main area should be focused.
        if self.overview_active {
            self.overview.set_focus(true);
            self.steps.set_focus(false);
        } else {
            self.steps.set_focus(true);
            self.overview.set_focus(false);
        }
        self.details.set_focus(false);
    }

    /// Focus the details section.
    pub fn focus_details(&mut self) {
        // The details pane is hidden while previewing a collection overview
        // (collections have no notes or comments), so keep focus on the
        // overview instead of landing on an invisible pane.
        if self.overview_active {
            self.focus_steps();
            return;
        }
        self.focused = RightPaneFocus::Details;
        self.details.set_focus(true);
        self.steps.set_focus(false);
        self.overview.set_focus(false);
    }

    /// Get the last rendered area.
    pub fn last_area(&self) -> Rect {
        self.last_area.get()
    }

    /// Scroll the main preview content by `delta` lines (positive scrolls down),
    /// regardless of focus. This drives the collection overview when one is
    /// active, otherwise the steps panel's active tab (code preview or info
    /// markdown). Returns true if the viewport moved.
    ///
    /// Lets `J`/`K` scroll the visible preview from anywhere in the browser
    /// without first focusing the right pane.
    pub fn scroll_main_content(&mut self, delta: i32) -> bool {
        if self.overview_active {
            self.overview.scroll_by(delta)
        } else {
            self.steps.scroll_active(delta)
        }
    }

    /// Toggle internal focus between Steps and Details.
    pub fn toggle_internal_focus(&mut self) {
        match self.focused {
            RightPaneFocus::Steps => self.focus_details(),
            RightPaneFocus::Details => self.focus_steps(),
        }
    }

    /// Whether the currently visible main preview panel supports link
    /// navigation (`n`/`N`). Only markdown panels render links; the code preview
    /// (Steps) and query (Query) tabs do not, so the status bar and help popup
    /// hide the Links binding while one of those is active.
    pub fn link_navigation_available(&self) -> bool {
        // The live collection overview is markdown and renders its links.
        if self.overview_active {
            return true;
        }
        match self.focused {
            // Both bottom tabs (Details/Comments) are markdown panels.
            RightPaneFocus::Details => matches!(
                self.details.panels.get(self.details.tabs.selected_index()),
                Some(crate::browser::TabContent::Markdown(_))
            ),
            // Among the steps tabs only the Info tab is markdown.
            RightPaneFocus::Steps => self.steps.get_markdown().is_some(),
        }
    }

    /// Get the markdown content from the currently focused markdown panel.
    /// Returns the content from the Details panel when focused on Details,
    /// or from the Info tab's markdown panel when the Info tab is selected.
    /// Returns None if there's no content or the preview state is cleared.
    pub fn active_markdown_content(&self) -> Option<&str> {
        // When showing a live collection overview, the markdown is the overview
        // itself (there are no per-step bookmarks loaded).
        if self.overview_active {
            return Some(self.overview.markdown()).filter(|m| !m.trim().is_empty());
        }

        // Return None if there are no steps loaded (preview state is cleared)
        if self.steps_data.is_empty() {
            return None;
        }

        let content = match self.focused {
            RightPaneFocus::Details => {
                if let Some(crate::browser::TabContent::Markdown(md)) =
                    self.details.panels.get(self.details.tabs.selected_index())
                {
                    Some(md.markdown())
                } else {
                    None
                }
            }
            RightPaneFocus::Steps => {
                // Only return markdown if the Info tab is selected
                if self.steps.tabs.selected_index() == INFO_TAB_INDEX {
                    self.steps.get_markdown().map(|m| m.markdown())
                } else {
                    None
                }
            }
        };

        // Filter out empty or whitespace-only content
        content.filter(|m| !m.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codemark_core::engine::bookmark::{BookmarkHealth, Visibility};
    use tempfile::TempDir;

    fn sample_bookmark(id: &str, query: &str, file_path: &str) -> Bookmark {
        Bookmark {
            id: id.to_string(),
            query: query.to_string(),
            language: "rust".to_string(),
            file_path: file_path.to_string(),
            content_hash: None,
            commit_hash: None,
            health: BookmarkHealth::Active,
            resolution_method: None,
            last_resolved_at: None,
            stale_since: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            current_resolution_id: None,
            repo_id: None,
            tags: Vec::new(),
            annotations: Vec::new(),
            comments: Vec::new(),
        }
    }

    fn sample_collection(id: &str, name: &str) -> codemark_core::engine::bookmark::Collection {
        codemark_core::engine::bookmark::Collection {
            id: id.to_string(),
            name: name.to_string(),
            description: None,
            visibility: Visibility::Private,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            created_by: None,
            created_branch: None,
            published_at: None,
            published_commit_sha: None,
            repo_url: None,
            repo_id: None,
            status: None,
            health: None,
            health_computed_at: None,
            updated_at: None,
            imported_from_url: None,
        }
    }

    /// Build a `RightPane` over a temp database seeded with a two-step collection.
    fn right_pane_with_collection() -> (RightPane, Database, TempDir) {
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("repo").join(".codemark").join("codemark.db");
        let db = Database::create(&db_path).expect("create db");
        db.insert_bookmark(&sample_bookmark("bm-1", "fn main", "src/main.rs")).unwrap();
        db.insert_bookmark(&sample_bookmark("bm-2", "struct Config", "src/config.rs")).unwrap();
        db.insert_collection(&sample_collection("col-1", "Tour")).unwrap();
        db.add_to_collection("col-1", &["bm-1".to_string(), "bm-2".to_string()]).unwrap();
        let pane = RightPane::new(&db);
        (pane, db, tmp)
    }

    #[test]
    fn paging_targets_the_new_step_for_inline_code_and_background_markdown() {
        let (mut pane, db, _tmp) = right_pane_with_collection();
        pane.load_tour(&db, "Tour");
        assert_eq!(pane.pager_total, 2);
        assert_eq!(pane.current_step_bookmark_id(), Some("bm-1"));
        assert_eq!(pane.current_step_render_input().map(|(bm, _)| bm.id), Some("bm-1".to_string()));

        // Simulate a pager move to the second step (as `RightPane::handle_event`
        // does) and refresh the inline code pane.
        pane.pager_current = 1;
        pane.update_step_code();

        // Both the staleness key and the background-render snapshot now point at
        // the second step: a late result for bm-1 is dropped, and the spawned
        // task renders bm-2's markdown.
        assert_eq!(pane.current_step_bookmark_id(), Some("bm-2"));
        assert_eq!(pane.current_step_render_input().map(|(bm, _)| bm.id), Some("bm-2".to_string()));
    }

    #[test]
    fn apply_step_markdown_updates_the_info_tab() {
        let (mut pane, db, _tmp) = right_pane_with_collection();
        pane.load_tour(&db, "Tour");

        pane.apply_step_markdown(crate::browser::StepPreviewMarkdown {
            info_markdown: "# INFO-MARKER".to_string(),
            details_markdown: "DETAILS-MARKER".to_string(),
            comments_markdown: "COMMENTS-MARKER".to_string(),
        });

        // `get_markdown` only returns the Info panel when its tab is active
        // (it defaults to the Steps/code tab), so select Info before reading.
        pane.steps.tabs.set_selected(INFO_TAB_INDEX);
        let info = pane.steps.get_markdown().map(|m| m.markdown().to_string()).unwrap_or_default();
        assert!(info.contains("INFO-MARKER"), "info tab should show applied markdown, got: {info}");
    }
}
