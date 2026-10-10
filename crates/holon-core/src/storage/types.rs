//! Shared storage abstractions used by every storage adapter (Turso, Loro, …).
//!
//! These types are deliberately adapter-agnostic: a `Filter` is expressed in
//! domain terms, `StorageEntity` is the row shape, and `StorageError`/`Result`
//! are the common error surface. The path
//! `#crate_path::storage::types::StorageEntity` is also hard-coded in the
//! `operations_trait` macro, so this layout is load-bearing.

use std::collections::HashMap;

use holon_api::Value;
use thiserror::Error;

/// StorageEntity type alias for `HashMap<Arc<str>, Value>`. //
/// ALLOW(compatibility): macro-emitted callers expect this exact path.
pub type StorageEntity = HashMap<std::sync::Arc<str>, Value>;

/// An adapter-agnostic query predicate over entity fields.
#[derive(Debug, Clone)]
pub enum Filter {
    Eq(String, Value),
    In(String, Vec<Value>),
    And(Vec<Filter>),
    Or(Vec<Filter>),
    IsNull(String),
    IsNotNull(String),
}

/// The common error surface for storage adapters.
#[derive(Debug, Error)]
pub enum StorageError {
    #[error("Entity not found: {entity} with id {id}")]
    NotFound { entity: String, id: String },

    #[error("Schema error: {0}")]
    SchemaError(String),

    #[error("Query error: {0}")]
    QueryError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error(transparent)]
    NotJson(#[from] holon_api::NotJson),

    #[error("Backend error: {0}")]
    BackendError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Database error: {0}")]
    DatabaseError(String),

    /// The engine refused to open a database file. Only `Corrupt` lets the
    /// open move the file aside; this error means the file is where it was.
    #[error("cannot open the database {path}: {failure}. Engine error: {cause}")]
    DatabaseUnopenable {
        path: String,
        failure: DatabaseOpenFailure,
        cause: String,
    },

    /// The database actor is gone, so no statement can reach the database any
    /// more. Only process teardown produces this, and it is its own variant
    /// because a caller running during teardown must tell it apart from a
    /// statement that failed against a LIVE database: the first is expected
    /// and the second is a defect. Message kept verbatim — the reboot and
    /// shutdown suites match it as text.
    #[error("Database error: Actor channel closed")]
    ActorGone,

    /// DDL naming resources that no schema provider registers. Distinct from a
    /// dependency *timeout*: nothing is coming, so the caller must disclose it
    /// now instead of parking.
    #[error(
        "missing dependencies {missing:?} — no schema provider registers them.\nSQL: {sql_preview}"
    )]
    MissingDependencies {
        sql_preview: String,
        missing: Vec<String>,
    },
}

pub type Result<T> = std::result::Result<T, StorageError>;

/// Why the engine could not open a database file, as far as it lets us tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DatabaseOpenFailure {
    /// Another process holds the file's lock.
    Locked,
    /// The file system refused an operation on the file.
    Io(std::io::ErrorKind),
    /// The file is not a database, or its header or pages are damaged.
    Corrupt,
    /// An engine error none of the above names.
    Unclassified,
}

impl std::fmt::Display for DatabaseOpenFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use std::io::ErrorKind;
        match self {
            Self::Locked => write!(
                f,
                "another process holds it open; quit the other Holon process that uses this \
                 database, then start Holon again"
            ),
            Self::Io(ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem) => write!(
                f,
                "no permission to write it; give this user write access to the file and its \
                 directory"
            ),
            Self::Io(ErrorKind::StorageFull | ErrorKind::QuotaExceeded) => {
                write!(f, "the disk is full; free disk space")
            }
            Self::Io(ErrorKind::NotFound) => {
                write!(f, "its directory does not exist; create the directory")
            }
            Self::Io(kind) => write!(f, "the file system refused it ({kind})"),
            Self::Corrupt => write!(f, "the file is damaged"),
            Self::Unclassified => write!(
                f,
                "the engine gave an error Holon does not classify, so the file is left as it is"
            ),
        }
    }
}
