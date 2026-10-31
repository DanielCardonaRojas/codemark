//! Error handling and exit codes for the codemark CLI.

use std::process;

/// Exit codes as defined in the CLI specification.
#[allow(dead_code)]
pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_OPERATION_FAILED: i32 = 1;
pub const EXIT_INPUT_ERROR: i32 = 2;
pub const EXIT_DATABASE_ERROR: i32 = 3;

/// Core error type for the codemark project.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum Error {
    /// Command or feature not yet implemented.
    #[error("{0}")]
    NotImplemented(String),

    /// Generic operation failure.
    #[error("operation failed: {0}")]
    Operation(String),

    /// Invalid user input or CLI arguments.
    #[error("input error: {0}")]
    Input(String),

    /// SQLite database error.
    #[error("database error: {0}")]
    Database(String),

    /// Thrown when the .codemark directory is missing.
    #[error(
        "not a codemark repository (or any of the parent directories): .codemark. Run 'codemark init' to initialize."
    )]
    NotInitialized,

    /// No grammar is registered for a language a bookmark refers to.
    ///
    /// Distinct from [`Error::Input`] because it is nearly always recoverable
    /// by installing the grammar, and because the two look identical to a UI
    /// otherwise: a bookmark whose grammar is missing is not broken, it is
    /// merely unreadable until the language is installed. Reporting it as a
    /// resolution failure makes a healthy collection look rotten.
    #[error(
        "no grammar installed for '{language}'. Install one with \
         `codemark languages add <grammar.wasm> --from-registry {language}`, \
         or see `codemark languages registry list` for what is available."
    )]
    GrammarNotInstalled { language: String },

    /// Error during tree-sitter parsing or query execution.
    #[error("tree-sitter error: {0}")]
    TreeSitter(String),

    /// Git command or git2 library error.
    #[error("git error: {0}")]
    Git(String),

    /// Bookmark resolution failed.
    #[error("resolution error: {0}")]
    Resolution(String),

    /// Query matched multiple distinct nodes.
    #[error("ambiguous query: {0}")]
    AmbiguousQuery(String),

    /// Standard I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// JSON serialization/deserialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<rusqlite::Error> for Error {
    fn from(err: rusqlite::Error) -> Self {
        Error::Database(err.to_string())
    }
}

impl From<git2::Error> for Error {
    fn from(err: git2::Error) -> Self {
        Error::Git(err.to_string())
    }
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::NotInitialized => EXIT_INPUT_ERROR,
            Error::NotImplemented(_) | Error::Operation(_) => EXIT_OPERATION_FAILED,
            Error::Input(_) | Error::Io(_) => EXIT_INPUT_ERROR,
            // The input was fine; the environment is missing a grammar. Treated
            // as operational so a script can tell "you asked for something
            // impossible" from "install this and retry".
            Error::GrammarNotInstalled { .. } => EXIT_OPERATION_FAILED,
            Error::Database(_) => EXIT_DATABASE_ERROR,
            Error::TreeSitter(_) | Error::Resolution(_) | Error::AmbiguousQuery(_) => {
                EXIT_OPERATION_FAILED
            }
            Error::Git(_) => EXIT_OPERATION_FAILED,
            Error::Json(_) => EXIT_OPERATION_FAILED,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Print an error and exit with the appropriate code.
pub fn exit_with_error(err: &Error) -> ! {
    eprintln!("codemark: {err}");
    process::exit(err.exit_code());
}
