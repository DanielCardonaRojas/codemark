//! CLI integration tests — runs the actual codemark binary end-to-end.
//!
//! Each test gets its own temp database for isolation.

use std::path::PathBuf;
use std::process::Command;

/// Helper to run codemark with a temp database.
struct Codemark {
    db_path: PathBuf,
    binary: PathBuf,
}

impl Codemark {
    fn new() -> Self {
        let db_dir = std::env::temp_dir().join(format!("codemark_test_{}", uuid()));
        std::fs::create_dir_all(&db_dir).unwrap();
        let db_path = db_dir.join("codemark.db");

        // Build path to the binary
        let binary = PathBuf::from(env!("CARGO_BIN_EXE_codemark"));

        Codemark { db_path, binary }
    }

    fn run(&self, args: &[&str]) -> CmdResult {
        let output = Command::new(&self.binary)
            .arg("--db")
            .arg(&self.db_path)
            .args(args)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("failed to run codemark");

        CmdResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            status: output.status.code().unwrap_or(-1),
        }
    }

    fn run_json(&self, args: &[&str]) -> serde_json::Value {
        let mut full_args = vec!["--format", "json"];
        full_args.extend_from_slice(args);
        let result = self.run(&full_args);
        assert_eq!(result.status, 0, "command failed: {}\n{}", result.stderr, result.stdout);
        serde_json::from_str(&result.stdout)
            .unwrap_or_else(|e| panic!("invalid JSON: {e}\nstdout: {}", result.stdout))
    }

    /// Fixture path relative to the project root.
    fn fixture(&self, name: &str) -> String {
        format!("tests/fixtures/{name}")
    }
}

impl Drop for Codemark {
    fn drop(&mut self) {
        if let Some(parent) = self.db_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

struct CmdResult {
    stdout: String,
    stderr: String,
    status: i32,
}

fn uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let c = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}{}_{}", t.as_secs(), t.subsec_nanos(), c)
}

// --- Tests ---

#[test]
fn add_and_resolve_round_trip() {
    let cm = Codemark::new();

    // Add a bookmark
    let json = cm.run_json(&[
        "add",
        "--file", &cm.fixture("swift/auth_service.swift"),
        "--range", "39",
        "--tag", "auth",
        "--note", "JWT validation",
    ]);
    assert_eq!(json["success"], true);
    let id = json["data"]["id"].as_str().unwrap().to_string();
    assert!(!id.is_empty());

    // Resolve it
    let json = cm.run_json(&["resolve", &id[..8]]);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["method"], "exact");
    assert!(json["data"]["file"].as_str().unwrap().contains("auth_service.swift"));
}

#[test]
fn add_dry_run_does_not_persist() {
    let cm = Codemark::new();

    // Dry run
    let json = cm.run_json(&[
        "add",
        "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108",
        "--dry-run",
    ]);
    assert_eq!(json["data"]["dry_run"], true);
    assert_eq!(json["data"]["node_type"], "function_item");

    // List should be empty
    let json = cm.run_json(&["list"]);
    let bookmarks = json["data"].as_array().unwrap();
    assert!(bookmarks.is_empty());
}

#[test]
fn add_auto_detects_language() {
    let cm = Codemark::new();

    // No --lang flag — should auto-detect from .rs extension
    let json = cm.run_json(&[
        "add",
        "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108",
        "--note", "auto-detect test",
    ]);
    assert_eq!(json["success"], true);
}

#[test]
fn add_with_created_by() {
    let cm = Codemark::new();

    cm.run_json(&[
        "add",
        "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108",
        "--created-by", "agent",
        "--note", "agent bookmark",
    ]);
    cm.run_json(&[
        "add",
        "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "47",
        "--created-by", "user",
        "--note", "user bookmark",
    ]);

    // Filter by author
    let json = cm.run_json(&["list", "--author", "agent"]);
    let bookmarks = json["data"].as_array().unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0]["notes"], "agent bookmark");

    let json = cm.run_json(&["list", "--author", "user"]);
    let bookmarks = json["data"].as_array().unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0]["notes"], "user bookmark");
}

#[test]
fn add_from_snippet() {
    let cm = Codemark::new();

    let output = Command::new(&cm.binary)
        .arg("--db")
        .arg(&cm.db_path)
        .arg("--format")
        .arg("json")
        .args(["add-from-snippet", "--file", &cm.fixture("rust/auth_service.rs"), "--tag", "auth"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(b"fn decode").unwrap();
            child.wait_with_output()
        })
        .unwrap();

    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["success"], true);
}

#[test]
fn list_with_filters() {
    let cm = Codemark::new();

    // Add bookmarks in different languages with different tags
    cm.run_json(&[
        "add", "--file", &cm.fixture("swift/auth_service.swift"),
        "--range", "39", "--tag", "auth", "--note", "swift auth",
    ]);
    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--tag", "factory", "--note", "rust factory",
    ]);

    // List all
    let json = cm.run_json(&["list"]);
    assert_eq!(json["data"].as_array().unwrap().len(), 2);

    // Filter by tag
    let json = cm.run_json(&["list", "--tag", "auth"]);
    assert_eq!(json["data"].as_array().unwrap().len(), 1);
    assert_eq!(json["data"][0]["notes"], "swift auth");

    // Filter by language
    let json = cm.run_json(&["list", "--lang", "rust"]);
    assert_eq!(json["data"].as_array().unwrap().len(), 1);
    assert_eq!(json["data"][0]["notes"], "rust factory");
}

#[test]
fn show_includes_resolution_history() {
    let cm = Codemark::new();

    let json = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "test",
    ]);
    let id = json["data"]["id"].as_str().unwrap().to_string();

    let json = cm.run_json(&["show", &id]);
    let data = &json["data"];
    assert_eq!(data["bookmark"]["id"], id);
    // Initial resolution should exist
    let resolutions = data["resolutions"].as_array().unwrap();
    assert!(!resolutions.is_empty());
    assert_eq!(resolutions[0]["method"], "exact");
}

#[test]
fn heal_updates_statuses() {
    let cm = Codemark::new();

    cm.run_json(&[
        "add", "--file", &cm.fixture("swift/auth_service.swift"),
        "--range", "39", "--note", "test",
    ]);
    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "test",
    ]);

    let json = cm.run_json(&["heal"]);
    assert_eq!(json["data"]["active"], 2);
    assert_eq!(json["data"]["drifted"], 0);
    assert_eq!(json["data"]["stale"], 0);
    assert_eq!(json["data"]["validate_only"], false);
}

#[test]
fn heal_validate_only_skips_recording() {
    let cm = Codemark::new();

    cm.run_json(&[
        "add", "--file", &cm.fixture("swift/auth_service.swift"),
        "--range", "39", "--note", "test",
    ]);

    let json = cm.run_json(&["heal", "--validate-only"]);
    assert_eq!(json["data"]["active"], 1);
    assert_eq!(json["data"]["validate_only"], true);
}

#[test]
fn status_shows_counts() {
    let cm = Codemark::new();

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "test",
    ]);

    let json = cm.run_json(&["status"]);
    assert_eq!(json["data"]["active"], 1);
}

#[test]
fn search_finds_by_note() {
    let cm = Codemark::new();

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "unique-search-term-xyz",
    ]);
    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "47", "--note", "something else",
    ]);

    let json = cm.run_json(&["search", "unique-search-term"]);
    let bookmarks = json["data"].as_array().unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert!(bookmarks[0]["notes"].as_str().unwrap().contains("unique-search-term"));
}

#[test]
fn search_finds_by_file_path() {
    let cm = Codemark::new();

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "test 1",
    ]);
    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/api_client.rs"),
        "--range", "1", "--note", "test 2",
    ]);

    // Search by file path component - should find api_client bookmark
    let json = cm.run_json(&["search", "api_client"]);
    let bookmarks = json["data"].as_array().unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert!(bookmarks[0]["file_path"].as_str().unwrap().contains("api_client"));
}

#[test]
fn search_finds_by_tag() {
    let cm = Codemark::new();

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--tag", "unique-tag-xyz", "--note", "test 1",
    ]);
    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "47", "--tag", "other", "--note", "test 2",
    ]);

    // Search by tag - should find the bookmark with unique-tag-xyz
    let json = cm.run_json(&["search", "unique-tag"]);
    let bookmarks = json["data"].as_array().unwrap();
    assert_eq!(bookmarks.len(), 1);
    // tags is serialized as a JSON array
    let tags = bookmarks[0]["tags"].as_array().unwrap();
    assert!(tags.iter().any(|t| t.as_str().unwrap().contains("unique-tag-xyz")));
}

#[test]
fn collections_crud() {
    let cm = Codemark::new();

    // Add a bookmark
    let json = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "test",
    ]);
    let bm_id = json["data"]["id"].as_str().unwrap().to_string();

    // Create collection
    let json = cm.run_json(&["collection", "create", "test-col", "--description", "Test"]);
    assert_eq!(json["success"], true);

    // Add bookmark to collection
    let json = cm.run_json(&["collection", "add", "test-col", &bm_id]);
    assert_eq!(json["success"], true);

    // List collections
    let json = cm.run_json(&["collection", "list"]);
    let collections = json["data"].as_array().unwrap();
    assert_eq!(collections.len(), 1);

    // Show collection contents
    let json = cm.run_json(&["collection", "show", "test-col"]);
    let bookmarks = json["data"].as_array().unwrap();
    assert_eq!(bookmarks.len(), 1);

    // Resolve collection
    let json = cm.run_json(&["collection", "resolve", "test-col"]);
    assert_eq!(json["success"], true);

    // Remove bookmark from collection
    let json = cm.run_json(&["collection", "remove", "test-col", &bm_id]);
    assert_eq!(json["success"], true);

    // Delete collection
    let json = cm.run_json(&["collection", "delete", "test-col"]);
    assert_eq!(json["success"], true);
}

#[test]
fn export_and_import() {
    let cm = Codemark::new();

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "export test",
    ]);

    // Export
    let result = cm.run(&["export"]);
    assert_eq!(result.status, 0);
    let exported: Vec<serde_json::Value> = serde_json::from_str(&result.stdout).unwrap();
    assert_eq!(exported.len(), 1);

    // Write export to temp file
    let export_path = cm.db_path.parent().unwrap().join("export.json");
    std::fs::write(&export_path, &result.stdout).unwrap();

    // Import into a fresh DB
    let cm2 = Codemark::new();
    let json = cm2.run_json(&["import", export_path.to_str().unwrap()]);
    assert_eq!(json["success"], true);

    // Verify
    let json = cm2.run_json(&["list"]);
    let bookmarks = json["data"].as_array().unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0]["notes"], "export test");
}

#[test]
fn gc_removes_archived() {
    let cm = Codemark::new();

    let json = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "gc test",
    ]);
    assert_eq!(json["success"], true);

    // GC dry run — nothing archived yet
    let result = cm.run(&["gc", "--dry-run"]);
    assert_eq!(result.status, 0);
    assert!(result.stdout.contains("Would remove 0"));
}

#[test]
fn preview_shows_code() {
    let cm = Codemark::new();

    let json = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "preview test",
    ]);
    let id = json["data"]["id"].as_str().unwrap().to_string();

    // Preview now outputs JSON with resolution data
    let result = cm.run_json(&["preview", &id[..8]]);
    assert_eq!(result["success"], true);
    // Should contain the file path and line range
    assert!(result["data"]["file_path"].as_str().unwrap().contains("auth_service.rs"));
    assert!(result["data"]["line_range"].is_string());
}

#[test]
fn preview_uses_cached_resolution_by_default() {
    let cm = Codemark::new();

    // Add a bookmark - this creates an initial resolution
    let json = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "cached test",
    ]);
    let id = json["data"]["id"].as_str().unwrap().to_string();

    // Preview should use cached resolution
    let result = cm.run_json(&["preview", &id[..8]]);
    assert_eq!(result["success"], true);
    // Should have line_range from the cached resolution
    assert!(result["data"]["line_range"].is_string());
}

#[test]
fn preview_shows_drifted_status() {
    let cm = Codemark::new();

    let json = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "status test",
    ]);
    let id = json["data"]["id"].as_str().unwrap().to_string();

    let result = cm.run_json(&["preview", &id[..8]]);
    assert_eq!(result["success"], true);
    // Should show the bookmark status
    assert!(result["data"]["status"].is_string());
}

#[test]
fn diff_runs_without_error() {
    let cm = Codemark::new();

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "diff test",
    ]);

    // diff should succeed (may find 0 affected since fixtures aren't in recent commits)
    let json = cm.run_json(&["diff", "--since", "HEAD~1"]);
    assert_eq!(json["success"], true);
}

#[test]
fn line_output_is_tab_separated() {
    let cm = Codemark::new();

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--tag", "auth", "--note", "line format test",
    ]);

    let result = cm.run(&["list", "--format", "line"]);
    assert_eq!(result.status, 0);
    let line = result.stdout.trim();
    let fields: Vec<&str> = line.split('\t').collect();
    assert_eq!(fields.len(), 5, "expected 5 tab-separated fields, got: {line}");
    // Fields: id, file, status, tags, note
    assert_eq!(fields[2], "active");
    assert_eq!(fields[3], "auth");
    assert!(fields[4].contains("line format test"));
}

#[test]
fn hunk_range_parsing() {
    let cm = Codemark::new();

    // Line 109 is inside create_default_auth_service (single line hunk)
    let json = cm.run_json(&[
        "add",
        "--file", &cm.fixture("rust/auth_service.rs"),
        "--hunk", "@@ -109,1 +109,1 @@",
        "--dry-run",
    ]);
    assert_eq!(json["data"]["dry_run"], true);
    assert_eq!(json["data"]["node_type"], "function_item");
}

#[test]
fn byte_range_backwards_compat() {
    let cm = Codemark::new();

    let json = cm.run_json(&[
        "add",
        "--file", &cm.fixture("swift/auth_service.swift"),
        "--range", "b811:1302",
        "--dry-run",
    ]);
    assert_eq!(json["data"]["dry_run"], true);
    assert!(json["data"]["name"].as_str().unwrap_or("").contains("validateToken"));
}

#[test]
fn multi_language_support() {
    let cm = Codemark::new();

    let languages = [
        ("swift/auth_service.swift", "39"),
        ("rust/auth_service.rs", "108"),
        ("typescript/auth_service.ts", "30"),
        ("python/auth_service.py", "36"),
        ("go/auth_service.go", "49"),
        ("java/AuthService.java", "69"),
        ("csharp/AuthService.cs", "40"),
        ("dart/auth_service.dart", "10"),
    ];

    for (fixture, line) in languages {
        let json = cm.run_json(&[
            "add", "--file", &cm.fixture(fixture), "--range", line, "--dry-run",
        ]);
        assert_eq!(
            json["success"], true,
            "failed for {fixture}: {}",
            json
        );
        assert_eq!(
            json["data"]["dry_run"], true,
            "failed for {fixture}"
        );
    }
}

#[test]
fn nonexistent_bookmark_returns_error() {
    let cm = Codemark::new();

    let result = cm.run(&["--json", "resolve", "nonexistent"]);
    assert_ne!(result.status, 0);
}

#[test]
fn invalid_file_returns_error() {
    let cm = Codemark::new();

    let result = cm.run(&["--json", "add", "--file", "nonexistent.rs", "--range", "1"]);
    assert_ne!(result.status, 0);
}

#[test]
fn remove_bookmark() {
    let cm = Codemark::new();

    // Add two bookmarks
    let json1 = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "first",
    ]);
    let id1 = json1["data"]["id"].as_str().unwrap().to_string();

    let json2 = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "47", "--note", "second",
    ]);
    let id2 = json2["data"]["id"].as_str().unwrap().to_string();

    // Remove the first one by prefix
    let result = cm.run(&["remove", &id1[..8]]);
    assert_eq!(result.status, 0);
    assert!(result.stdout.contains("Removed 1 bookmark"));

    // Only the second should remain
    let json = cm.run_json(&["list"]);
    let bookmarks = json["data"].as_array().unwrap();
    assert_eq!(bookmarks.len(), 1);
    assert_eq!(bookmarks[0]["id"], id2);
}

#[test]
fn remove_multiple_bookmarks() {
    let cm = Codemark::new();

    let json1 = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "a",
    ]);
    let id1 = json1["data"]["id"].as_str().unwrap().to_string();

    let json2 = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "47", "--note", "b",
    ]);
    let id2 = json2["data"]["id"].as_str().unwrap().to_string();

    // Remove both at once
    let result = cm.run(&["remove", &id1[..8], &id2[..8]]);
    assert_eq!(result.status, 0);
    assert!(result.stdout.contains("Removed 2 bookmarks"));

    let json = cm.run_json(&["list"]);
    let bookmarks = json["data"].as_array().unwrap();
    assert!(bookmarks.is_empty());
}

#[test]
fn remove_nonexistent_returns_error() {
    let cm = Codemark::new();

    let result = cm.run(&["remove", "nonexistent"]);
    assert_ne!(result.status, 0);
}

#[test]
fn collection_ordering_and_reorder() {
    let cm = Codemark::new();

    // Add 3 bookmarks
    let j1 = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"), "--range", "108", "--note", "step-1",
    ]);
    let id1 = j1["data"]["id"].as_str().unwrap().to_string();

    let j2 = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"), "--range", "47", "--note", "step-2",
    ]);
    let id2 = j2["data"]["id"].as_str().unwrap().to_string();

    let j3 = cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"), "--range", "65", "--note", "step-3",
    ]);
    let id3 = j3["data"]["id"].as_str().unwrap().to_string();

    // Create collection and add in order
    cm.run_json(&["collection", "create", "callpath"]);
    cm.run_json(&["collection", "add", "callpath", &id1, &id2, &id3]);

    // Show should respect insertion order
    let json = cm.run_json(&["collection", "show", "callpath"]);
    let bookmarks = json["data"].as_array().unwrap();
    assert_eq!(bookmarks[0]["notes"], "step-1");
    assert_eq!(bookmarks[1]["notes"], "step-2");
    assert_eq!(bookmarks[2]["notes"], "step-3");

    // Reorder: step-3, step-1, step-2
    cm.run_json(&["collection", "reorder", "callpath", &id3, &id1, &id2]);

    let json = cm.run_json(&["collection", "show", "callpath"]);
    let bookmarks = json["data"].as_array().unwrap();
    assert_eq!(bookmarks[0]["notes"], "step-3");
    assert_eq!(bookmarks[1]["notes"], "step-1");
    assert_eq!(bookmarks[2]["notes"], "step-2");
}

// --- Semantic search tests ---

#[test]
fn semantic_search_finds_by_meaning() {
    let cm = Codemark::new();

    // Add bookmarks with related meanings but different keywords
    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "user authentication function",
    ]);

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "47", "--note", "login validation",
    ]);

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "65", "--note", "database query",
    ]);

    // Run reindex to generate embeddings
    cm.run(&["reindex"]);

    // Search for "auth" should find both authentication and login bookmarks
    let json = cm.run_json(&["search", "auth", "--semantic"]);
    let results = json["data"].as_array().unwrap();

    // Should find at least the bookmarks related to authentication
    assert!(results.len() >= 2, "Expected at least 2 results for semantic 'auth' search");

    // All results should have distance scores
    for result in results {
        assert!(result.get("distance").is_some(), "Each result should have a distance score");
    }
}

#[test]
fn semantic_search_without_reindex_fails_gracefully() {
    let cm = Codemark::new();

    // Add a bookmark
    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "test bookmark",
    ]);

    // Don't run reindex - semantic search should still work but might not find results
    // or return an error message
    let result = cm.run(&["search", "test", "--semantic"]);
    // The command should succeed (status 0) even if no embeddings exist
    assert_eq!(result.status, 0);
}

#[test]
fn semantic_search_with_limit() {
    let cm = Codemark::new();

    // Add multiple bookmarks
    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "first",
    ]);

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "47", "--note", "second",
    ]);

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "65", "--note", "third",
    ]);

    // Run reindex
    cm.run(&["reindex"]);

    // Search with limit
    let json = cm.run_json(&["search", "test", "--semantic", "--limit", "2"]);
    let results = json["data"].as_array().unwrap();

    // Should return at most 2 results
    assert!(results.len() <= 2, "Results should be limited to 2");
}

#[test]
fn reindex_generates_embeddings() {
    let cm = Codemark::new();

    // Add some bookmarks
    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "test bookmark 1",
    ]);

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "47", "--note", "test bookmark 2",
    ]);

    // Reindex should succeed (may fail if model download issues in CI)
    let result = cm.run(&["reindex"]);
    // Note: reindex may fail if model download fails in test environment
    // but the bookmark creation should still work
    if result.status == 0 {
        assert!(result.stdout.contains("Generated embeddings"));
    }
}

// --- Heal protection tests (backward healing prevention) ---

#[test]
fn heal_validates_without_resolution() {
    // Test that heal works normally when there's no prior resolution
    let cm = Codemark::new();

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "no resolution test",
    ]);

    // Heal should work normally (no resolution to check against)
    let json = cm.run_json(&["heal"]);
    assert_eq!(json["data"]["active"], 1);
    assert_eq!(json["data"]["skipped"], 0);
}

#[test]
fn heal_force_flag_exists() {
    // Test that --force flag is accepted (doesn't error)
    let cm = Codemark::new();

    cm.run_json(&[
        "add", "--file", &cm.fixture("rust/auth_service.rs"),
        "--range", "108", "--note", "force flag test",
    ]);

    // Should accept --force without error
    let json = cm.run_json(&["heal", "--force"]);
    assert_eq!(json["success"], true);
    assert_eq!(json["data"]["active"], 1);
    assert_eq!(json["data"]["skipped"], 0);
}

// Note: Testing actual backward healing prevention requires setting up
// a git repository with multiple commits and checking out old commits,
// which is complex for integration tests. The unit tests in src/git/context.rs
// verify the is_ancestor function works correctly, and the integration
// tests verify the --force flag is accepted and heal works normally.

