//! Core data types for holon API
//!
//! This module defines the types used across all frontends (Tauri, Flutter, etc.)
//! to interact with the holon backend.

// Traversal and NewBlock now live in holon_api::repository
pub use holon_api::repository::{NewBlock, Traversal};
