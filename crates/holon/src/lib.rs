pub mod api;
pub mod core;
pub mod di;
pub mod entity_profile;
pub mod navigation;
pub mod petri;
pub mod render_dsl;
pub mod storage;
pub mod sync;
#[cfg(not(target_arch = "wasm32"))]
pub mod testing;
pub mod util;

// Re-export macro-generated operation dispatch modules for HasCache trait
#[cfg(not(target_arch = "wasm32"))]
pub use core::datasource::__operations_has_cache;
