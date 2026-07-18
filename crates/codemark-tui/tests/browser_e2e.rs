//! End-to-end UI tests for the browser layout.
//!
//! These tests drive [`BrowserLayout`] the same way the real event loop does —
//! by feeding it [`Event`]s and rendering into a ratatui [`TestBackend`] — but
//! without a terminal or PTY. The rendered buffer is captured as text and
//! snapshotted with `insta`, so a test reads as "feed these keys → the screen
//! looks like this".
//!
//! ## Sandboxing
//!
//! [`BrowserLayout::new`] opens the *global* registry (resolved from
//! `CODEMARK_DATA_DIR` / `XDG_*`) and the per-repo database. To keep tests
//! hermetic we point every directory at a fresh `TempDir` via [`Sandbox`], so a
//! test never reads or writes the developer's real codemark state.
//!
//! ## What this covers (and what it doesn't)
//!
//! `BrowserLayout` owns nearly all of the TUI's behavior, so testing it
//! directly gives high coverage. The top-level glue in `main.rs` (the global
//! `q`/`?`/`/`/`Esc` keys and the mode/focus transitions) lives outside the
//! layout and is *not* exercised here — if that grows, extract it into a
//! testable function and add cases below.

use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use codemark_core::engine::bookmark::{Bookmark, BookmarkHealth};
use codemark_core::storage::db::Database;
use codemark_tui::browser::{BrowserLayout, FocusArea};
use codemark_tui::event::{Event, EventHandler, EventHandlerConfig};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempfile::TempDir;

/// Process-wide lock guarding the environment variables that select the global
/// registry/config directories. Tests mutate `std::env`, which is global and
/// not thread-safe, so they must run one at a time. `cargo test` runs tests in
/// parallel by default; this serializes only the env-sensitive setup.
fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
}

/// A fully sandboxed codemark environment: a temp directory standing in for the
/// global data/config/cache dirs, plus an isolated per-repo database.
///
/// Holds the env lock for its lifetime so concurrent tests don't clobber each
/// other's environment. Fields drop in declaration order (`db` → `_tmp` →
/// `_guard`), which is what we want: the database is closed before its
/// `TempDir` is removed, and the env lock is released last.
struct Sandbox {
    db: Database,
    _tmp: TempDir,
    _guard: MutexGuard<'static, ()>,
}

impl Sandbox {
    /// Build a sandbox with an empty database.
    fn new() -> Self {
        let guard = env_lock();
        let tmp = TempDir::new().expect("create temp dir");

        // Point every global-state lookup at the sandbox. These are read by
        // `codemark_core::config::{global_data_dir, global_config_dir}` and
        // honored on all platforms.
        // SAFETY: env access is serialized by the held `env_lock` guard.
        unsafe {
            std::env::set_var("CODEMARK_DATA_DIR", tmp.path().join("data"));
            std::env::set_var("XDG_CONFIG_HOME", tmp.path().join("config"));
            std::env::set_var("XDG_DATA_HOME", tmp.path().join("data"));
            std::env::set_var("XDG_CACHE_HOME", tmp.path().join("cache"));
        }

        let db_path = tmp.path().join("repo").join(".codemark").join("codemark.db");
        let db = Database::create(&db_path).expect("create sandbox database");

        Self { db, _tmp: tmp, _guard: guard }
    }

    /// Build a sandbox pre-seeded with `bookmarks`.
    fn with_bookmarks(bookmarks: impl IntoIterator<Item = Bookmark>) -> Self {
        let sandbox = Self::new();
        for bm in bookmarks {
            sandbox.db.insert_bookmark(&bm).expect("seed bookmark");
        }
        sandbox
    }

    /// Absolute path to the sandbox repo root (the directory the database's
    /// `.codemark/` lives in). Bookmark `file_path`s resolve relative to this.
    fn repo_root(&self) -> std::path::PathBuf {
        self._tmp.path().join("repo")
    }

    /// Write a source file into the sandbox repo at `rel_path` (relative to the
    /// repo root). Seeding the real file makes live resolution succeed so the
    /// preview pane renders deterministic content instead of a machine-specific
    /// "could not load file /var/folders/..." error path.
    fn write_repo_file(&self, rel_path: &str, contents: &str) {
        let full = self.repo_root().join(rel_path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("create source dir");
        }
        std::fs::write(full, contents).expect("write source file");
    }

    /// Write a layered config that makes the layout consider itself "logged in"
    /// (a named server carrying a token resolves to usable credentials), so the
    /// login-gated Tours tab is visible. Config lives alongside the database in
    /// `repo/.codemark/`, which is where `Config::load_layered` reads it from.
    fn write_logged_in_config(&self) {
        let codemark_dir = self.repo_root().join(".codemark");
        std::fs::create_dir_all(&codemark_dir).expect("create .codemark dir");
        std::fs::write(
            codemark_dir.join("config.toml"),
            "[codetours]\n\
             default_server = \"remote\"\n\n\
             [[codetours.servers]]\n\
             name = \"remote\"\n\
             url = \"http://example.com\"\n\
             token = \"test-token\"\n",
        )
        .expect("write config.toml");
    }
}

/// A minimal valid bookmark for seeding. `id`/`query`/`file_path` are the
/// fields that surface in the list UI; everything else is a sensible default.
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

/// Construct a `BrowserLayout` over the sandbox database. Requires a tokio
/// runtime in scope because `BrowserLayout::new` spawns a background live-health
/// task — annotate tests with `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`.
fn make_layout(sandbox: Sandbox) -> (BrowserLayout, Sandbox) {
    // Drop the receiver: tests that don't pump self-sent events don't need it.
    let (layout, sandbox, _rx) = make_layout_with_rx(sandbox);
    (layout, sandbox)
}

/// Like [`make_layout`], but also returns the event handler's receiver so a test
/// can pump events the layout sends to itself (e.g. the `SearchResults` that an
/// FTS `execute_search` posts back through the channel).
fn make_layout_with_rx(
    sandbox: Sandbox,
) -> (BrowserLayout, Sandbox, tokio::sync::mpsc::Receiver<Event>) {
    // A real event handler; its background event loop polls a terminal we never
    // touch in tests, but `BrowserLayout` needs a handle to send custom events.
    // Disable mouse capture and bracketed paste: the default config writes their
    // enable escape sequences to stdout (with no matching cleanup), which would
    // leave an interactive shell in a broken state after the test process exits.
    let (rx, handler) = EventHandler::with_receiver(
        EventHandlerConfig::default()
            .tick_rate(Duration::from_millis(100))
            .enable_mouse(false)
            .enable_paste(false),
    )
    .expect("event handler");

    // `Database` isn't `Clone`, so move it out of the sandbox into the layout
    // and rebuild the sandbox shell to keep the env/tempdir alive for the test.
    let Sandbox { db, _tmp, _guard } = sandbox;
    let layout = BrowserLayout::new(db, handler);
    // Reopen a handle so callers can still query the DB if needed. Cheap: same
    // file, fresh connection.
    let db_path = _tmp.path().join("repo").join(".codemark").join("codemark.db");
    let db = Database::open(&db_path).expect("reopen sandbox database");
    (layout, Sandbox { db, _tmp, _guard }, rx)
}

/// Drain any events the layout has posted to its own channel and feed them back
/// through `handle_event`, mimicking one turn of the real event loop. The
/// unbounded→bounded forwarding runs on a spawned task, so give it a moment to
/// deliver before draining.
async fn pump_pending_events(
    layout: &mut BrowserLayout,
    rx: &mut tokio::sync::mpsc::Receiver<Event>,
) {
    tokio::time::sleep(Duration::from_millis(50)).await;
    while let Ok(event) = rx.try_recv() {
        layout.handle_event(&event);
    }
}

/// Feed a single character key.
fn key_char(layout: &mut BrowserLayout, c: char) -> bool {
    layout.handle_event(&Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)))
}

/// Feed an arbitrary key code (e.g. `KeyCode::Down`, `KeyCode::Enter`).
/// Part of the harness toolkit; not every test uses it yet.
#[allow(dead_code)]
fn key_code(layout: &mut BrowserLayout, code: KeyCode) -> bool {
    layout.handle_event(&Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
}

/// Feed a string of characters in order (e.g. typing into a filter).
/// Part of the harness toolkit; not every test uses it yet.
#[allow(dead_code)]
fn type_str(layout: &mut BrowserLayout, s: &str) {
    for c in s.chars() {
        key_char(layout, c);
    }
}

/// Render the layout into a fixed-size `TestBackend` and return the screen as a
/// newline-joined string of trimmed rows — stable input for snapshotting.
fn render_to_string(layout: &BrowserLayout, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|f| {
            layout.render(f.area(), f.buffer_mut());
        })
        .expect("draw");

    let buffer = terminal.backend().buffer();
    let mut lines = Vec::with_capacity(height as usize);
    for y in 0..height {
        let mut line = String::with_capacity(width as usize);
        for x in 0..width {
            line.push_str(buffer[(x, y)].symbol());
        }
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn empty_database_renders_and_focuses_repos_pane() {
    let sandbox = Sandbox::new();
    let (layout, _sandbox) = make_layout(sandbox);

    // With no bookmarks, the layout starts focused on the repos pane so the
    // user can pick a repository (see `BrowserLayout::new`).
    assert_eq!(layout.focus(), FocusArea::ContextPanel);

    insta::assert_snapshot!(render_to_string(&layout, 100, 30));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn seeded_bookmarks_render_in_list() {
    let sandbox = Sandbox::with_bookmarks([
        sample_bookmark("bm-1", "fn main", "src/main.rs"),
        sample_bookmark("bm-2", "struct Config", "src/config.rs"),
    ]);
    // Seed the real source files so live resolution succeeds and the preview is
    // deterministic (no absolute temp path leaking into the snapshot).
    sandbox.write_repo_file("src/main.rs", "fn main() {\n    println!(\"hi\");\n}\n");
    sandbox.write_repo_file("src/config.rs", "struct Config {\n    name: String,\n}\n");
    let (layout, _sandbox) = make_layout(sandbox);

    // A non-empty database focuses the bookmarks pane on startup.
    assert_eq!(layout.focus(), FocusArea::ContentPanel);

    insta::assert_snapshot!(render_to_string(&layout, 100, 30));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn filtering_narrows_the_bookmark_list() {
    let sandbox = Sandbox::with_bookmarks([
        sample_bookmark("bm-1", "fn main", "src/main.rs"),
        sample_bookmark("bm-2", "struct Config", "src/config.rs"),
    ]);
    sandbox.write_repo_file("src/main.rs", "fn main() {\n    println!(\"hi\");\n}\n");
    sandbox.write_repo_file("src/config.rs", "struct Config {\n    name: String,\n}\n");
    let (mut layout, _sandbox) = make_layout(sandbox);

    // `apply_filter` is exactly what the real event loop calls each keystroke
    // (main.rs computes the focused panel's active filter and applies it). With
    // the bookmarks pane focused, this narrows the *list* to matching entries:
    // the snapshot's bookmarks pane shows only `src/config.rs` ("1 of 1").
    //
    // Note: `apply_filter` only calls `set_filter` on the panel; it does not
    // refresh the right-hand preview pane (that happens on selection change, not
    // on filter). So the preview still shows `src/main.rs` from layout init.
    // That stale-preview state is expected here and is what the snapshot captures.
    layout.apply_filter("config");

    insta::assert_snapshot!(render_to_string(&layout, 100, 30));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_filter_survives_focus_regained() {
    let sandbox = Sandbox::with_bookmarks([
        sample_bookmark("bm-1", "fn main", "src/main.rs"),
        sample_bookmark("bm-2", "struct Config", "src/config.rs"),
    ]);
    sandbox.write_repo_file("src/main.rs", "fn main() {\n    println!(\"hi\");\n}\n");
    sandbox.write_repo_file("src/config.rs", "struct Config {\n    name: String,\n}\n");
    let (mut layout, _sandbox, mut rx) = make_layout_with_rx(sandbox);

    // Focus the search bar ('s'), type a query, and run it (Enter). The default
    // FTS mode searches synchronously and posts the results back through the
    // event channel, which `pump_pending_events` then applies.
    key_char(&mut layout, 's');
    type_str(&mut layout, "config");
    key_code(&mut layout, KeyCode::Enter);
    pump_pending_events(&mut layout, &mut rx).await;

    // The bookmark list is narrowed to the single match: only `src/config.rs`
    // shows and the full-list footer ("… of 2") is gone.
    let filtered = render_to_string(&layout, 100, 30);
    assert!(
        filtered.contains("config.rs") && !filtered.contains("of 2"),
        "search should narrow the bookmark list to the match; got:\n{filtered}"
    );

    // Switching to another app and back emits FocusGained, which refreshes the
    // panels from the DB. That used to silently drop the search while its text
    // stayed in the search bar. The search-active list is now preserved in place
    // (no full-list rebuild, so no flicker), so the filter must still hold.
    layout.handle_event(&Event::FocusGained);

    let after = render_to_string(&layout, 100, 30);
    assert!(
        after.contains("config.rs") && !after.contains("of 2"),
        "search filter should survive FocusGained (the full list must not return); got:\n{after}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_results_reconcile_with_db_on_focus_regained() {
    // Two bookmarks match a "config" path search; one of them will be deleted
    // out from under the TUI (as the CLI might while the terminal is unfocused).
    let sandbox = Sandbox::with_bookmarks([
        sample_bookmark("bm-1", "fn main", "src/main.rs"),
        sample_bookmark("bm-2", "struct Config", "src/config.rs"),
        sample_bookmark("bm-3", "fn helper", "src/config_helper.rs"),
    ]);
    sandbox.write_repo_file("src/main.rs", "fn main() {\n    println!(\"hi\");\n}\n");
    sandbox.write_repo_file("src/config.rs", "struct Config {\n    name: String,\n}\n");
    sandbox.write_repo_file("src/config_helper.rs", "fn helper() {}\n");
    let (mut layout, sandbox, mut rx) = make_layout_with_rx(sandbox);

    // Run the search; both config paths match (FTS matches on file_path).
    key_char(&mut layout, 's');
    type_str(&mut layout, "config");
    key_code(&mut layout, KeyCode::Enter);
    pump_pending_events(&mut layout, &mut rx).await;

    let filtered = render_to_string(&layout, 100, 30);
    assert!(
        filtered.contains("config.rs") && filtered.contains("config_helper.rs"),
        "both config bookmarks should show before the deletion; got:\n{filtered}"
    );

    // Delete one match while the TUI is "unfocused", then regain focus. The
    // preserved rows still list the deleted bookmark, but the refocus reconcile
    // prunes rows whose bookmark no longer exists in the DB, so the stale row
    // disappears in place instead of lingering until the next full refresh.
    assert!(sandbox.db.delete_bookmark("bm-3").expect("delete bookmark"));
    layout.handle_event(&Event::FocusGained);

    let after = render_to_string(&layout, 100, 30);
    assert!(
        after.contains("config.rs") && !after.contains("config_helper.rs"),
        "the deleted bookmark should be reconciled away after refocus; got:\n{after}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_reconcile_prunes_results_on_an_inactive_tab() {
    // Regression for the case where a search-active panel is not the tab shown
    // at refocus: reconcile must target the panel that owns the search results,
    // not only the selected tab.
    let sandbox = Sandbox::with_bookmarks([
        sample_bookmark("bm-1", "fn main", "src/main.rs"),
        sample_bookmark("bm-2", "struct Config", "src/config.rs"),
        sample_bookmark("bm-3", "fn helper", "src/config_helper.rs"),
    ]);
    sandbox.write_repo_file("src/main.rs", "fn main() {\n    println!(\"hi\");\n}\n");
    sandbox.write_repo_file("src/config.rs", "struct Config {\n    name: String,\n}\n");
    sandbox.write_repo_file("src/config_helper.rs", "fn helper() {}\n");
    let (mut layout, sandbox, mut rx) = make_layout_with_rx(sandbox);

    // Search bookmarks (both config paths match), which leaves focus on the
    // Content panel with the Bookmarks tab narrowed.
    key_char(&mut layout, 's');
    type_str(&mut layout, "config");
    key_code(&mut layout, KeyCode::Enter);
    pump_pending_events(&mut layout, &mut rx).await;

    // Switch to the Collections tab: the narrowed Bookmarks panel is now hidden
    // but still search-active.
    key_char(&mut layout, ']');

    // Delete one bookmark match while unfocused, regain focus, then switch back
    // to the (previously hidden) Bookmarks tab. The stale row must be gone: the
    // reconcile prunes the search-active Bookmarks panel even though Collections
    // was the selected tab when focus returned.
    assert!(sandbox.db.delete_bookmark("bm-3").expect("delete bookmark"));
    layout.handle_event(&Event::FocusGained);
    key_char(&mut layout, '[');

    let after = render_to_string(&layout, 100, 30);
    assert!(
        after.contains("config.rs") && !after.contains("config_helper.rs"),
        "the deleted bookmark should be pruned even though its tab was hidden at refocus; got:\n{after}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_tour_overview_survives_a_panel_refresh() {
    use codemark_core::sync::RemoteTourSummary;

    let sandbox = Sandbox::with_bookmarks([sample_bookmark("bm-1", "fn main", "src/main.rs")]);
    sandbox.write_repo_file("src/main.rs", "fn main() {}\n");
    // Logged-in so the Tours tab is visible/selectable.
    sandbox.write_logged_in_config();
    let (mut layout, _sandbox) = make_layout(sandbox);

    // Seeded bookmark => focus starts on Content panel. Cycle to the Tours tab.
    assert_eq!(layout.focus(), FocusArea::ContentPanel);
    key_char(&mut layout, ']'); // Bookmarks -> Collections
    key_char(&mut layout, ']'); // Collections -> Tours

    // Deliver a remote tour for the unscoped (no active repos) fetch. The default
    // `pending_remote_repos` is `None`, so a `None` scope is accepted.
    let tour = RemoteTourSummary {
        tour_id: "tour-xyz".to_string(),
        title: "RemoteOnboarding".to_string(),
        repo_url: Some("https://github.com/acme/widgets".to_string()),
        author: Some("octocat".to_string()),
        updated_at: "2026-01-01T00:00:00Z".to_string(),
    };
    layout.handle_event(&Event::RemoteToursLoaded(vec![tour], None));

    // The remote tour overview should render in the right pane.
    let before = render_to_string(&layout, 140, 40);
    assert!(
        before.contains("RemoteOnboarding"),
        "remote tour overview should render after load; got:\n{before}"
    );

    // A panel refresh (pull/sync/db-switch/etc.) must not drop the remote preview.
    layout.refresh_all_panels();
    let after = render_to_string(&layout, 140, 40);
    assert!(
        after.contains("RemoteOnboarding"),
        "remote tour overview should survive refresh_all_panels; got:\n{after}"
    );
}
