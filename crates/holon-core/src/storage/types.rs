//! Re-export storage types so the operations_trait macro can resolve them. // ALLOW(compatibility): module exists to satisfy the macro's import path
//!
//! The path `#crate_path::storage::types::StorageEntity` is hard-coded in
//! `operations_trait`, so this file mirrors that layout.

use holon_api::Value;
use std::collections::HashMap;

/// StorageEntity type alias for `HashMap<String, Value>`. // ALLOW(compatibility): macro-emitted callers expect this exact path.
pub type StorageEntity = HashMap<String, Value>;
