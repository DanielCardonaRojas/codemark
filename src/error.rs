use std::process;

/// Exit codes as defined in the CLI specification.
#[allow(dead_code)]
pub const EXIT_SUCCESS: i32 = 0;
pub const EXIT_OPERATION_FAILED: i32 = 1;
pub const EXIT_INPUT_ERROR: i32 = 2;
pub const EXIT_DATABASE_ERROR: i32 = 3;

#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum Error {
    #[error("{0}")]
    NotImplemented(String),

    #[error("operation failed: {0}")]
    Operation(String),

    #[error("input error: {0}")]
    Input(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("tree-sitter error: {0}")]
    TreeSitter(String),

    #[error("git error: {0}")]
    Git(String),

    #[error("resolution error: {0}")]
    Resolution(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

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
            Error::NotImplemented(_) | Error::Operation(_) => EXIT_OPERATION_FAILED,
            Error::Input(_) | Error::Io(_) => EXIT_INPUT_ERROR,
            Error::Database(_) => EXIT_DATABASE_ERROR,
            Error::TreeSitter(_) | Error::Resolution(_) => EXIT_OPERATION_FAILED,
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
