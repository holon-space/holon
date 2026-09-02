//! @c4 component
//! @c4 layer Adapters
//! Pattern: Adapter
//! @c4 uses holon "core orchestration" "Rust"
//! @c4 uses holon-loro "Loro CRDT backend & P2P sync" "Rust"
//! @c4 uses holon-turso "Turso storage adapter" "Rust"
//! @c4 uses holon-sharing "ADR 0028 policy overlay" "Rust"
//!
//! The wiring that binds the Loro CRDT backend to the `holon` engine.
//!
//! Everything here needs BOTH `holon` and `holon-loro`; nothing in `holon`
//! needs it back. Keeping it in its own crate is what lets `holon` compile in
//! parallel with `holon-loro` instead of waiting for it.

pub mod event_infra_module;
pub mod loro_block_query_source;
pub mod loro_module;
pub mod loro_ui_watcher;
pub mod memory_backend;
// Pulls in proptest, which is native-only. Gated behind `testing` (or
// `#[cfg(test)]`): zero production consumers.
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "testing")))]
pub mod pbt_infrastructure;

pub use event_infra_module::EventInfraModule;
pub use loro_module::LoroConfig;
pub use loro_module::LoroModule;
pub use loro_module::block_sql_write_provider;
pub use memory_backend::MemoryBackend;
