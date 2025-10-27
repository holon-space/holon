//! Dependency Injection module for holon
//!
//! This module provides service registration and resolution using fluxdi.
//! It centralizes dependency wiring and makes it easier to test and configure services.
//!
//! Submodules:
//! - `runtime`: Async utilities for DI factories
//! - `lifecycle`: App lifecycle functions (creating/initializing BackendEngine)
//! - `registration`: DI service registration functions

pub mod lifecycle;
pub mod registration;
pub mod runtime;
pub mod schema_providers;
#[cfg(any(test, feature = "test-helpers"))]
pub mod test_helpers;

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::storage::turso::{DbHandle, TursoBackend};

// Re-export public API
pub use lifecycle::{
    CoreInfraModule, create_backend_engine, create_backend_engine_with_extras,
    open_and_register_core, preload_startup_views,
};
pub use registration::{register_core_services, register_core_services_with_backend};
pub use runtime::{create_queryable_cache, create_queryable_cache_async, run_async_in_sync};
pub use schema_providers::{DbReady, DbResource};

/// Trait for providing the TursoBackend.
///
/// This trait allows DI resolution using string-based keys instead of TypeId,
/// avoiding TypeId mismatches across crates.
pub trait TursoBackendProvider: Send + Sync {
    fn backend(&self) -> Arc<RwLock<TursoBackend>>;
}

pub(crate) struct TursoBackendProviderImpl {
    pub(crate) backend: Arc<RwLock<TursoBackend>>,
}

impl TursoBackendProvider for TursoBackendProviderImpl {
    fn backend(&self) -> Arc<RwLock<TursoBackend>> {
        self.backend.clone()
    }
}

/// Trait for providing the DbHandle for the database actor.
///
/// This trait allows DI resolution of the database actor handle across crates.
pub trait DbHandleProvider: Send + Sync {
    fn handle(&self) -> DbHandle;
}

pub(crate) struct DbHandleProviderImpl {
    pub(crate) handle: DbHandle,
}

impl DbHandleProvider for DbHandleProviderImpl {
    fn handle(&self) -> DbHandle {
        self.handle.clone()
    }
}

/// Common PRQL queries used during app startup.
///
/// These queries are pre-compiled into materialized views BEFORE file watching
/// or data sync starts. This eliminates the "database is locked" bug that occurs
/// when `query_and_watch` tries to CREATE MATERIALIZED VIEW while IVM is busy
/// processing incoming data.
///
/// IMPORTANT: Only context-independent queries can be preloaded here.
pub const STARTUP_QUERIES: &[&str] = &[
    include_str!("../../sql/startup/preload_blocks.prql"),
    include_str!("../../sql/startup/preload_text_blocks.prql"),
];

/// Configuration for database path
#[derive(Clone, Debug)]
pub struct DatabasePathConfig {
    pub path: PathBuf,
}

impl DatabasePathConfig {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}
