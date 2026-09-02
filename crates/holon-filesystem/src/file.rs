use holon_api::BatchMetadata;
use holon_api::Change;
use holon_api::WithMetadata;
use holon_macros::Entity;
use serde::Deserialize;
use serde::Serialize;

/// Changes wrapped with metadata for atomic sync token updates
pub type ChangesWithMetadata<T> = WithMetadata<Vec<Change<T>>, BatchMetadata>;

/// File - represents a file in the filesystem that maps to a logical Document
#[derive(Debug, Clone, Serialize, Deserialize, Entity)]
#[entity(name = "file", short_name = "file", graph_label = "file")]
pub struct File {
    #[primary_key]
    #[indexed]
    pub id: String,

    /// Filename (e.g. "index.org")
    pub name: String,

    /// Relative path of the containing folder, from the vault root — e.g.
    /// `Projects/DBG`, or `.` for a file sitting directly in the root.
    ///
    /// This is a plain path string, NOT an entity id and NOT a foreign key:
    /// nothing joins on it. Keep it a `String` — the deleted `Directory` entity
    /// wrapped the same relative path in an `EntityUri`, and because
    /// `EntityUri::from_raw` maps an unschemed string to `block:<s>`, a folder
    /// named `Agentic DPL` became `block:Agentic DPL` and panicked at boot (a
    /// space is not a legal RFC 3986 URI character). A path is not an entity
    /// id; giving this field a URI type would reintroduce that bug.
    #[indexed]
    pub parent_id: String,

    /// SHA256 for change detection
    pub content_hash: String,

    /// FK to Document.id (UUID), None until adapter creates the Document
    #[indexed]
    pub document_id: Option<String>,

    /// Overflow bag for properties with no column of their own; the engine's
    /// `_provenance` stamp lands here.
    #[jsonb]
    #[value_kind(overflow_properties)]
    pub properties: Option<String>,

    /// Per-key kind map for `properties`, holding an entry only where the JSON
    /// form is ambiguous. NULL means every key reads back at its JSON-evident
    /// kind.
    #[value_kind(overflow_property_kinds)]
    pub property_kinds: Option<String>,
}

impl File {
    pub fn new(
        id: String,
        name: String,
        parent_id: String,
        content_hash: String,
        document_id: Option<String>,
    ) -> Self {
        Self {
            id,
            name,
            parent_id,
            content_hash,
            document_id,
            properties: None,
            property_kinds: None,
        }
    }
}
