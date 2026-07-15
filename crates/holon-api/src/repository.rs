//! DocumentRepository trait and related types
//!
//! This module defines the repository pattern interface for interacting
//! with hierarchical block documents. All frontends use this trait.
//!
//! # Trait Architecture
//!
//! The API is split into 4 focused traits that backends can implement
//! selectively:
//!
//! - `CoreOperations`: CRUD and batch operations (required for all backends)
//! - `Lifecycle`: Document creation and disposal (required for all backends)
//! - `ChangeNotifications`: Real-time state sync and change streams
//! - `P2POperations`: Peer-to-peer networking and synchronization
//!
//! The `DocumentRepository` supertrait combines all four for convenience.
//! Backends implementing all four automatically satisfy `DocumentRepository`.

use async_trait::async_trait;
use serde::Deserialize;
use serde::Serialize;

use crate::streaming::ChangeNotifications;
use crate::ApiError;
use crate::Block;
use crate::BlockContent;
use crate::EntityUri;

/// Configuration for filtering blocks by tree depth when traversing.
///
/// Depth levels:
/// - Level 0: Root of the current document
/// - Level 1: Top-level user blocks (direct children of root)
/// - Level 2+: Nested blocks
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Traversal {
    /// Minimum depth level to include (inclusive)
    pub min_level: usize,
    /// Maximum depth level to include (inclusive)
    pub max_level: usize,
}

impl Traversal {
    /// Only top-level user blocks (level 1)
    pub const TOP_LEVEL: Self = Self {
        min_level: 1,
        max_level: 1,
    };

    /// All blocks including the synthetic root (levels 0 to MAX)
    pub const ALL: Self = Self {
        min_level: 0,
        max_level: usize::MAX,
    };

    /// All non-root blocks (levels 1 to MAX) — the most common traversal mode
    pub const ALL_BUT_ROOT: Self = Self {
        min_level: 1,
        max_level: usize::MAX,
    };

    /// Custom depth range
    pub const fn new(min_level: usize, max_level: usize) -> Self {
        Self {
            min_level,
            max_level,
        }
    }

    /// Check if a given depth level should be included
    pub const fn includes_level(&self, level: usize) -> bool {
        level >= self.min_level && level <= self.max_level
    }
}

/// Template for creating a new block in batch operations.
///
/// # Positioning
///
/// - `after = None`: Insert at start of parent's children
/// - `after = Some(sibling_id)`: Insert after specified sibling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewBlock {
    /// Parent block/document URI
    pub parent_id: EntityUri,
    /// Initial content (text, source block, etc.)
    pub content: BlockContent,
    /// Position anchor: insert after this sibling (None = insert at start)
    pub after: Option<EntityUri>,
    /// Optional custom ID (None = generate local URI)
    pub id: Option<EntityUri>,
}

impl NewBlock {
    /// Create a new text block
    pub fn text(parent_id: EntityUri, text: impl Into<String>) -> Self {
        Self {
            parent_id,
            content: BlockContent::text(text),
            after: None,
            id: None,
        }
    }

    /// Create a new source block
    pub fn source(
        parent_id: EntityUri,
        language: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            parent_id,
            content: BlockContent::source(language, source),
            after: None,
            id: None,
        }
    }

    /// Create a new image block. `path` is the relative file path.
    pub fn image(parent_id: EntityUri, path: impl Into<String>) -> Self {
        Self {
            parent_id,
            content: BlockContent::image(path),
            after: None,
            id: None,
        }
    }

    /// Builder: set position after a sibling
    pub fn after(mut self, sibling_id: EntityUri) -> Self {
        self.after = Some(sibling_id);
        self
    }

    /// Builder: set custom ID
    pub fn with_id(mut self, id: EntityUri) -> Self {
        self.id = Some(id);
        self
    }
}

/// Core CRUD and batch operations for block documents.
///
/// This trait provides the fundamental operations for creating, reading,
/// updating, and deleting blocks. All backends must implement this trait.
#[async_trait]
pub trait CoreOperations: Send + Sync {
    // ===== Single-Block Operations =====

    /// Get a block by ID.
    async fn get_block(&self, id: &str) -> Result<Block, ApiError>;

    /// Get ancestor chain by traversing parent_id links.
    ///
    /// Returns a vector of parent IDs from immediate parent up to (but not
    /// including) the root. Stops when encountering a sentinel URI
    /// (`EntityUri::is_no_parent()`) or the root block itself.
    async fn get_ancestor_chain(&self, id: &str) -> Result<Vec<String>, ApiError> {
        let mut ancestors = Vec::new();
        let mut current_id = id.to_string();

        loop {
            let block = self.get_block(&current_id).await?;

            if block.parent_id.is_no_parent() {
                break;
            }

            ancestors.push(block.parent_id.to_string());
            current_id = block.parent_id.to_string();
        }

        Ok(ancestors)
    }

    /// Get all non-deleted blocks in tree order, filtered by depth.
    async fn get_all_blocks(&self, traversal: Traversal) -> Result<Vec<Block>, ApiError>;

    /// List children IDs of a block in display order.
    async fn list_children(&self, parent_id: &str) -> Result<Vec<String>, ApiError>;

    /// Create a new block.
    async fn create_block(
        &self,
        parent_id: EntityUri,
        content: BlockContent,
        id: Option<EntityUri>,
    ) -> Result<Block, ApiError>;

    /// Update block content.
    async fn update_block(&self, id: &str, content: BlockContent) -> Result<(), ApiError>;

    /// Delete a block (tombstone).
    async fn delete_block(&self, id: &str) -> Result<(), ApiError>;

    /// Move block to new parent and position.
    async fn move_block(
        &self,
        id: &EntityUri,
        new_parent: EntityUri,
        after: Option<EntityUri>,
    ) -> Result<(), ApiError>;

    // ===== Block-Based Convenience Methods =====

    /// Update block content using a block reference.
    async fn update_block_by_ref(
        &self,
        block: &Block,
        content: BlockContent,
    ) -> Result<(), ApiError> {
        self.update_block(block.id.as_str(), content).await
    }

    /// Delete a block using a block reference.
    async fn delete_block_by_ref(&self, block: &Block) -> Result<(), ApiError> {
        self.delete_block(block.id.as_str()).await
    }

    /// Move block to new parent and position using block references.
    async fn move_block_by_ref(
        &self,
        block: &Block,
        new_parent: Option<&Block>,
        after: Option<&Block>,
    ) -> Result<(), ApiError> {
        let parent_id = new_parent
            .map(|p| p.id.clone())
            .unwrap_or_else(|| block.parent_id.clone());
        let after_id = after.map(|a| a.id.clone());

        self.move_block(&block.id, parent_id, after_id).await
    }

    // ===== Batch Operations =====

    /// Get multiple blocks by ID.
    async fn get_blocks(&self, ids: Vec<String>) -> Result<Vec<Block>, ApiError>;

    /// Create multiple blocks in a single transaction.
    async fn create_blocks(&self, blocks: Vec<NewBlock>) -> Result<Vec<Block>, ApiError>;

    /// Delete multiple blocks in a single transaction.
    async fn delete_blocks(&self, ids: Vec<String>) -> Result<(), ApiError>;
}

/// Document lifecycle management.
///
/// This trait handles creating new documents, opening existing ones,
/// and resource cleanup. All backends must implement this trait.
#[async_trait]
pub trait Lifecycle: Send + Sync {
    /// Create a new document.
    async fn create_new(doc_id: String) -> Result<Self, ApiError>
    where
        Self: Sized;

    /// Open an existing document.
    async fn open_existing(doc_id: String) -> Result<Self, ApiError>
    where
        Self: Sized;

    /// Dispose of document and release resources.
    async fn dispose(&self) -> Result<(), ApiError>;
}

/// Peer-to-peer networking and synchronization.
///
/// This trait provides P2P connectivity for distributed document
/// synchronization. Backends that support networking implement this trait.
#[async_trait]
pub trait P2POperations: Send + Sync {
    /// Get this node's P2P identifier.
    async fn get_node_id(&self) -> String;

    /// Connect to a remote peer for P2P synchronization.
    async fn connect_to_peer(&self, peer_node_id: String) -> Result<(), ApiError>;

    /// Start accepting incoming P2P connections.
    async fn accept_connections(&self) -> Result<(), ApiError>;
}

/// Complete repository interface combining all capabilities.
///
/// This is a convenience supertrait that combines `CoreOperations`,
/// `Lifecycle`, `ChangeNotifications`, and `P2POperations`. Any type
/// implementing all four automatically satisfies this trait via the blanket
/// implementation.
pub trait DocumentRepository:
    CoreOperations + Lifecycle + ChangeNotifications<Block> + P2POperations
{
}

/// Blanket implementation: any type implementing all four traits automatically
/// implements DocumentRepository.
impl<T> DocumentRepository for T where
    T: CoreOperations + Lifecycle + ChangeNotifications<Block> + P2POperations
{
}
