//! @c4 component
//! @c4 layer Core
//! Pattern: Facade
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-engine "Petri-net engine" "Rust"
//! @c4 uses holon-macros "entity/operation derive macros" "Rust"
//! @c4 uses holon-turso "Turso storage adapter" "Rust"
//! @c4 uses holon-loro "Loro CRDT backend & P2P sync" "Rust"
//! @c4 uses holon-petri "Petri-net task materialization" "Rust"
//! @c4 uses holon-profiles "entity profile resolution" "Rust"
//!
//! Main orchestration crate: sync pipeline (Loro, OrgMode, Iroh), storage API,
//! BackendEngine, and DI modules.

pub mod api;
pub mod core;
pub mod di;
pub use holon_profiles as entity_profile;
pub mod identity;
pub mod navigation;
pub use holon_petri as petri;
pub mod storage;
pub mod sync;
// `testing` depends on proptest which is native-only (not on any wasm target)
// and gated behind the `testing` feature so proptest stays out of prod builds.
#[cfg(all(not(target_arch = "wasm32"), feature = "testing"))]
pub mod testing;
pub mod util;

// Re-export macro-generated operation dispatch modules for HasCache trait
pub use core::queryable_cache::__operations_has_cache;
