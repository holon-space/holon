//! Re-export datasource types so the operations_trait macro can resolve them.
//! // ALLOW(compatibility): module exists to satisfy the macro's import path
//!
//! The path `#crate_path::core::datasource::UnknownOperationError` is
//! hard-coded in `operations_trait`, so this file mirrors that layout.

pub use crate::Result;
pub use crate::UnknownOperationError;
