//! Directory entity for filesystem hierarchies

use async_trait::async_trait;
use holon_api::EntityUri;
use holon_core::traits::MaybeSendSync;
use holon_macros::Entity;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

use holon_api::streaming::ChangeNotifications;
use holon_api::OperationDescriptor;
use holon_api::StorageEntity;
use holon_api::{ApiError, Change, StreamPosition};
use holon_api::{BatchMetadata, EntityName, Value, WithMetadata};
use holon_core::traits::{
    CrudOperations, DataSource, OperationProvider, OperationRegistry, OperationResult,
    RenameOperations, Result,
};

/// Synthetic root ID for top-level directories
pub const ROOT_ID: &str = "null";

/// Directory - represents a folder in a directory hierarchy
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "directory", short_name = "dir", graph_label = "directory")]
pub struct Directory {
    #[primary_key]
    #[indexed]
    pub id: EntityUri,

    /// Directory name (relative to parent)
    pub name: String,

    /// Parent directory ID (ROOT_ID for top-level directories)
    #[indexed]
    pub parent_id: EntityUri,

    /// Nesting level from root (0 for root children)
    pub depth: i64,
}

impl Directory {
    pub fn new(id: String, name: String, parent_id: String, depth: i64) -> Self {
        Self {
            // ALLOW(entity_uri_from_raw): construction boundary — `Directory::new` takes raw `String` ids from the org sync provider; parse once here.
            id: EntityUri::from_raw(&id),
            name,
            // ALLOW(entity_uri_from_raw): construction boundary — see `id` above.
            parent_id: EntityUri::from_raw(&parent_id),
            depth,
        }
    }
}

/// Move operations (for entities with hierarchical structure)
///
/// This trait provides a move operation for entities that can be moved within
/// a hierarchical structure, such as directories, files, or blocks.
#[holon_macros::operations_trait]
#[async_trait]
pub trait DirectoryOperations: MaybeSendSync {
    #[holon_macros::affects("id", "parent_id", "depth")]
    #[holon_macros::triggered_by(availability_of = "dir_id", providing = ["parent_id"])]
    async fn move_directory(&self, id: &str, parent_id: &str) -> Result<OperationResult>;
}

impl holon_core::traits::BlockEntity for Directory {
    fn id(&self) -> &EntityUri {
        &self.id
    }

    fn parent_id(&self) -> Option<&EntityUri> {
        Some(&self.parent_id)
    }

    fn depth(&self) -> i64 {
        self.depth
    }

    fn content(&self) -> &str {
        &self.name
    }
    fn tags(&self) -> holon_api::Tags {
        holon_api::Tags::default()
    }
}

impl holon_core::traits::OperationRegistry for Directory {
    fn all_operations() -> Vec<OperationDescriptor> {
        let entity_name = Self::entity_name();
        let short_name = Self::short_name().expect("Directory must have short_name");
        let table = entity_name;
        let id_column = "id";

        #[cfg(not(target_arch = "wasm32"))]
        {
            use holon_core::traits::{
                __operations_block_operations, __operations_crud_operations,
                __operations_move_operations, __operations_rename_operations,
            };
            __operations_crud_operations::crud_operations(entity_name, short_name, table, id_column)
                .into_iter()
                .chain(__operations_block_operations::block_operations(
                    entity_name,
                    short_name,
                    table,
                    id_column,
                ))
                .chain(__operations_rename_operations::rename_operations(
                    entity_name,
                    short_name,
                    table,
                    id_column,
                ))
                .chain(__operations_move_operations::move_operations(
                    entity_name,
                    short_name,
                    table,
                    id_column,
                ))
                .collect()
        }
        #[cfg(target_arch = "wasm32")]
        {
            Vec::new()
        }
    }

    fn entity_name() -> &'static str {
        "directory"
    }

    fn short_name() -> Option<&'static str> {
        Directory::short_name()
    }
}

/// Changes wrapped with metadata for atomic sync token updates
pub type ChangesWithMetadata<T> = WithMetadata<Vec<Change<T>>, BatchMetadata>;

/// Trait for providers that can supply directory change streams
#[async_trait]
pub trait DirectoryChangeProvider: Send + Sync {
    fn subscribe_directories(&self) -> broadcast::Receiver<ChangesWithMetadata<Directory>>;

    /// Get the root directory path for this provider
    fn root_directory(&self) -> std::path::PathBuf;
}
