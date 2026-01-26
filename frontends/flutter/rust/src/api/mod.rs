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

pub use holon::api::types::{NewBlock, Traversal};
pub use holon::api::BackendEngine;
pub use holon::storage::turso::RowChangeStream;
pub use holon::storage::types::StorageEntity;
pub use holon_api::ApiError;
// Note: Block is NOT re-exported here - it comes directly from holon_api via FRB config
// to avoid duplicate class generation in Dart
pub use holon_api::{BlockChange, BlockMetadata};
pub use holon_api::{Change, ChangeOrigin, MapChange, StreamPosition};
pub use holon_api::{OperationDescriptor, OperationParam, RenderSpec};
