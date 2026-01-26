pub mod ffi_bridge;
pub mod flutter_mutation_driver;
pub mod shared_pbt;
pub mod types;

/// Stub module kept only so the auto-generated `frb_generated.rs` compiles.
/// Delete this module after regenerating FRB bindings.
pub mod flutter_pbt_backend {
    use flutter_rust_bridge::frb;

    #[frb(opaque)]
    pub struct FlutterPbtBackend;
}

pub use holon::api::BackendEngine;
pub use holon::api::types::NewBlock;
pub use holon::api::types::Traversal;
pub use holon::storage::turso::RowChangeStream;
pub use holon::storage::types::StorageEntity;
pub use holon_api::ApiError;
pub use holon_api::Change;
pub use holon_api::ChangeOrigin;
pub use holon_api::MapChange;
pub use holon_api::OperationDescriptor;
pub use holon_api::OperationParam;
pub use holon_api::StreamPosition;
// Note: Block is NOT re-exported here - it comes directly from holon_api via FRB config
// to avoid duplicate class generation in Dart
pub use holon_api::{BlockChange, BlockMetadata};
