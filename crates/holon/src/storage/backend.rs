//! The `StorageBackend` trait now lives in `holon-core`. Re-exported here so
//! the `crate::storage::backend::StorageBackend` (and `crate::storage::*`)
//! call sites keep resolving unchanged.

pub use holon_core::storage::backend::StorageBackend;
