//! Integration tests for external subcommand dispatch (the Git/Cargo plugin
//! model): `codemark <name>` runs `codemark-<name>`, e.g. `codemark tui` runs
//! `codemark-tui`.
//!
//! On Unix the dispatcher `exec()`s the plugin, so these tests run the real
//! `codemark` binary as a subprocess and assert on its stdout/stderr/exit code.
//! Plugin binaries are faked with small shell scripts on a controlled `PATH`,
//! which keeps the tests hermetic and independent of what is installed.

#![cfg(unix)]

use std::path::PathBuf;
use std::process::Command;

/// Path to the freshly-built `codemark` binary under test.
fn codemark_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_codemark"))
}

/// Create an isolated directory to act as the sole entry on `PATH`, so only the
/// fake plugins we install are discoverable.
fn plugin_dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// Write an executable shell script named `name` into `dir` that echoes the
/// arguments it received and exits with `exit_code`.
fn install_fake_plugin(dir: &std::path::Path, name: &str, exit_code: i32) {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join(name);
    let script = format!("#!/bin/sh\nprintf 'PLUGIN ARGS: %s\\n' \"$*\"\nexit {exit_code}\n");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Output {
    stdout: String,
    stderr: String,
    code: i32,
}

/// Run `codemark <args>` with `PATH` restricted to `path_dir`.
fn run_with_path(path_dir: &std::path::Path, args: &[&str]) -> Output {
    let out = Command::new(codemark_bin())
        .args(args)
        .env("PATH", path_dir)
        .output()
        .expect("failed to run codemark");
    Output {
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
        code: out.status.code().unwrap_or(-1),
    }
}

#[test]
fn builtin_command_is_not_treated_as_external() {
    // `--version` is handled by clap itself and must not be routed externally.
    let dir = plugin_dir();
    let out = run_with_path(dir.path(), &["--version"]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("codemark"), "stdout: {}", out.stdout);
}

#[test]
fn tui_dispatches_to_codemark_tui_binary() {
    let dir = plugin_dir();
    install_fake_plugin(dir.path(), "codemark-tui", 0);

    let out = run_with_path(dir.path(), &["tui"]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("PLUGIN ARGS:"), "stdout: {}", out.stdout);
}

#[test]
fn generic_plugin_dispatches_to_codemark_prefixed_binary_and_forwards_args() {
    let dir = plugin_dir();
    install_fake_plugin(dir.path(), "codemark-sync", 0);

    let out = run_with_path(dir.path(), &["sync", "--flag", "value"]);
    assert_eq!(out.code, 0, "stderr: {}", out.stderr);
    assert!(out.stdout.contains("PLUGIN ARGS: --flag value"), "stdout: {}", out.stdout);
}

#[test]
fn non_utf8_arguments_are_forwarded_not_rejected() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let dir = plugin_dir();
    install_fake_plugin(dir.path(), "codemark-sync", 0);

    // 0xFF is not valid UTF-8. With a `Vec<String>` external subcommand, clap
    // would reject this at parse time; `Vec<OsString>` forwards it to the plugin.
    let bad_arg = OsString::from_vec(vec![b'a', 0xFF, b'b']);
    let out = Command::new(codemark_bin())
        .arg("sync")
        .arg(&bad_arg)
        .env("PATH", dir.path())
        .output()
        .expect("failed to run codemark");

    let code = out.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("PLUGIN ARGS:"), "stdout: {}", stdout);
}

#[test]
fn plugin_exit_code_is_propagated() {
    let dir = plugin_dir();
    install_fake_plugin(dir.path(), "codemark-sync", 42);

    let out = run_with_path(dir.path(), &["sync"]);
    assert_eq!(out.code, 42, "stderr: {}", out.stderr);
}

#[test]
fn missing_tui_executable_shows_install_hint() {
    // Empty PATH dir -> codemark-tui cannot be found.
    let dir = plugin_dir();
    let out = run_with_path(dir.path(), &["tui"]);

    assert_eq!(out.code, 127, "stdout: {}", out.stdout);
    assert!(out.stderr.contains("TUI is not installed"), "stderr: {}", out.stderr);
    assert!(out.stderr.contains("cargo install codemark-tui"), "stderr: {}", out.stderr);
}

#[test]
fn missing_generic_plugin_shows_unknown_command_error() {
    let dir = plugin_dir();
    let out = run_with_path(dir.path(), &["does-not-exist"]);

    assert_eq!(out.code, 127, "stdout: {}", out.stdout);
    assert!(out.stderr.contains("Unknown command: does-not-exist"), "stderr: {}", out.stderr);
    assert!(out.stderr.contains("codemark-does-not-exist"), "stderr: {}", out.stderr);
}
