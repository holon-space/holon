//! Shared storage types now live in `holon-core`. Re-exported here so the
//! many `crate::storage::types::*` call sites (and the `operations_trait`
//! macro path) keep resolving unchanged.

pub use holon_core::storage::types::{Filter, Result, StorageEntity, StorageError};
