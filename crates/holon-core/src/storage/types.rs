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

    #[error("Backend error: {0}")]
    BackendError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Database error: {0}")]
    DatabaseError(String),
}

pub type Result<T> = std::result::Result<T, StorageError>;
