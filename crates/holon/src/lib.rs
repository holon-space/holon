pub mod api;
pub mod computed;
pub mod core;
pub mod di;
pub mod entity_profile;
pub mod identity;
pub mod navigation;
pub mod petri;
pub mod render_dsl;
pub mod storage;
pub mod sync;
// `testing` depends on proptest which is native-only (not on any wasm target).
#[cfg(not(target_arch = "wasm32"))]
pub mod testing;
pub mod type_registry;
pub mod util;

// Re-export macro-generated operation dispatch modules for HasCache trait
pub use core::datasource::__operations_has_cache;
