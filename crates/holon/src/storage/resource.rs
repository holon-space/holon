//! `Resource` now lives in `holon-core`. Re-exported here so the
//! `crate::storage::resource::Resource` call sites keep resolving unchanged.

pub use holon_core::storage::resource::Resource;
