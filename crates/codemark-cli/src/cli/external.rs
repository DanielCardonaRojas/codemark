//! External subcommand dispatch (Git/Cargo plugin model).
//!
//! A `codemark <subcommand>` that isn't a built-in command is dispatched to an
//! external executable named `codemark-<subcommand>`, forwarding all trailing
//! arguments. This mirrors how `git foo` runs `git-foo` and `cargo fmt` runs
//! `cargo-fmt`, and lets new plugins ship as standalone binaries without
//! changing the core CLI. For example, `codemark tui` launches the
//! `codemark-tui` binary.
//!
//! Dispatch is intentionally cheap and side-effect free up to the point of
//! launching the child, so it can run *before* config/database initialization
//! (see `main.rs`).

use std::ffi::{OsStr, OsString};

use crate::cli::Command;

/// Executable backing the `tui` subcommand. Special-cased only to provide a
/// friendlier "not installed" hint, since it's an optional component.
const TUI_EXECUTABLE: &str = "codemark-tui";

/// Exit code used when the external executable cannot be found (mirrors the
/// shell convention of 127 for "command not found").
const EXIT_NOT_FOUND: i32 = 127;

/// Exit code used when the executable exists but could not be launched (mirrors
/// the shell convention of 126 for "cannot execute").
const EXIT_LAUNCH_FAILED: i32 = 126;

/// A failure to dispatch an external subcommand.
///
/// Carries a fully-formed, user-facing message (printed verbatim to stderr) and
/// the process exit code to terminate with.
pub struct DispatchError {
    pub message: String,
    pub code: i32,
}

/// Resolve the external executable name for a subcommand, per the generic
/// plugin convention `codemark-<subcommand>`.
pub fn executable_name(subcommand: &str) -> String {
    format!("codemark-{subcommand}")
}

/// Dispatch a parsed command if it is external (a plugin such as `tui`),
/// forwarding its trailing arguments to the backing executable.
///
/// Returns:
/// - `None` if `command` is a built-in command (the caller should continue
///   normal startup).
/// - `Some(DispatchError)` if the command is external but the executable could
///   not be launched.
///
/// On success this **never returns**: on Unix it `exec()`s, replacing the
/// current process; on other platforms it waits for the child and exits with
/// the child's status code.
pub fn try_dispatch(command: &Command) -> Option<DispatchError> {
    let Command::External(parts) = command else {
        return None;
    };

    // clap guarantees at least the subcommand name is present, but guard
    // defensively rather than panic on an empty vector.
    let (subcommand, args) = parts.split_first()?;
    Some(run(subcommand, args))
}

/// Launch the external executable for `subcommand`, forwarding `args`.
///
/// Diverges on success; returns a [`DispatchError`] otherwise.
fn run(subcommand: &OsStr, args: &[OsString]) -> DispatchError {
    // The executable name must be valid UTF-8, so the subcommand is decoded
    // lossily here; the raw `args` are still forwarded verbatim as `OsString`.
    let subcommand = subcommand.to_string_lossy();
    let exe = executable_name(&subcommand);
    let mut command = std::process::Command::new(&exe);
    command.args(args);
    exec_or_spawn(command, &exe, &subcommand)
}

/// On Unix, replace the current process with the external executable so that
/// stdio, exit code, and signal handling are inherited natively.
#[cfg(unix)]
fn exec_or_spawn(mut command: std::process::Command, exe: &str, subcommand: &str) -> DispatchError {
    use std::os::unix::process::CommandExt;
    // `exec` only returns if it failed to replace the process image.
    let err = command.exec();
    spawn_error(err, exe, subcommand)
}

/// On non-Unix platforms, spawn the child, wait for it, and propagate its exit
/// code (there is no `exec` equivalent).
#[cfg(not(unix))]
fn exec_or_spawn(mut command: std::process::Command, exe: &str, subcommand: &str) -> DispatchError {
    match command.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(EXIT_LAUNCH_FAILED)),
        Err(err) => spawn_error(err, exe, subcommand),
    }
}

/// Translate a spawn/exec failure into a user-facing [`DispatchError`].
fn spawn_error(err: std::io::Error, exe: &str, subcommand: &str) -> DispatchError {
    if err.kind() == std::io::ErrorKind::NotFound {
        DispatchError { message: not_found_message(exe, subcommand), code: EXIT_NOT_FOUND }
    } else {
        DispatchError {
            message: format!("codemark: failed to launch {exe}: {err}"),
            code: EXIT_LAUNCH_FAILED,
        }
    }
}

/// Build the "executable not found" message, special-casing the TUI so it
/// points users at the right install command.
fn not_found_message(exe: &str, subcommand: &str) -> String {
    if exe == TUI_EXECUTABLE {
        format!(
            "The TUI is not installed.\n\n\
             Install it with:\n\n    cargo install {exe}"
        )
    } else {
        format!(
            "Unknown command: {subcommand}\n\n\
             No built-in command or external executable named:\n\n    {exe}"
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subcommands_map_to_codemark_prefixed_binary() {
        assert_eq!(executable_name("tui"), "codemark-tui");
        assert_eq!(executable_name("export"), "codemark-export");
        assert_eq!(executable_name("sync"), "codemark-sync");
    }

    #[test]
    fn tui_not_found_message_points_at_install_command() {
        let msg = not_found_message("codemark-tui", "tui");
        assert!(msg.contains("TUI is not installed"), "{msg}");
        assert!(msg.contains("cargo install codemark-tui"), "{msg}");
    }

    #[test]
    fn generic_plugin_not_found_message_names_the_executable() {
        let msg = not_found_message("codemark-export", "export");
        assert!(msg.contains("Unknown command: export"), "{msg}");
        assert!(msg.contains("codemark-export"), "{msg}");
    }
}
