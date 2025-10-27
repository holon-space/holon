//! Document entity for the blocks/documents data model.
//!
//! Documents are containers for blocks. Each document maps to a file on disk
//! (e.g., `todo.org`) and can have child documents (stored in a folder with
//! the same name, e.g., `todo/`).

use holon_macros::Entity;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::EntityUri;
use crate::Value;

/// Root document ID — `doc:__root__`
pub fn root_doc_id() -> EntityUri {
    EntityUri::doc_root()
}

/// No-parent sentinel — `sentinel:no_parent`
pub fn no_parent_doc_id() -> EntityUri {
    EntityUri::no_parent()
}

/// A document in the hierarchical document tree.
///
/// Documents correspond to files on disk and contain blocks. The document
/// hierarchy mirrors the filesystem structure:
/// - Document "projects" → file `projects.org`, folder `projects/`
/// - Document "projects/todo" → file `projects/todo.org`
///
/// # Path Derivation
/// The filesystem path is derived from the document hierarchy:
/// - `name` provides the filename stem
/// - `parent_id` chain provides the directory path
///
/// # Root Document
/// The root document has:
/// - `id = ROOT_DOC_ID` ("__root_doc__")
/// - `parent_id = NO_PARENT_DOC_ID` ("__no_parent__")
/// - `name = ""` (empty, represents the configured root directory)
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Entity)]
#[entity(
    name = "document",
    short_name = "doc",
    api_crate = "crate",
    graph_label = "Document"
)]
pub struct Document {
    /// Unique identifier (UUID or ULID)
    #[primary_key]
    #[indexed]
    pub id: EntityUri,

    /// Parent document ID. Root document uses NO_PARENT_DOC_ID.
    #[indexed]
    #[reference(Document, edge = "DOC_CHILD_OF")]
    pub parent_id: EntityUri,

    /// Display name / filename stem (e.g., "todo", "projects")
    /// Used to derive the filesystem path. Must be unique within parent.
    #[indexed]
    pub name: String,

    /// Fractional index for ordering within parent (lexicographic sort)
    /// Uses same algorithm as blocks (e.g., "a0", "a1", "a1V")
    pub sort_key: String,

    /// Key-value metadata (title, todo_keywords, custom properties)
    /// Stored as JSONB in the database.
    #[serde(default)]
    #[jsonb]
    pub properties: HashMap<String, Value>,

    /// Creation timestamp (Unix milliseconds)
    pub created_at: i64,

    /// Last update timestamp (Unix milliseconds)
    pub updated_at: i64,
}

impl Document {
    /// Create a new document with the given ID and name under the specified parent.
    pub fn new(id: EntityUri, parent_id: EntityUri, name: String) -> Self {
        let now = chrono::Utc::now().timestamp_millis();
        Self {
            id,
            parent_id,
            name,
            sort_key: "a0".to_string(), // Default sort key
            properties: HashMap::new(),
            created_at: now,
            updated_at: now,
        }
    }

    /// Create the root document.
    pub fn root() -> Self {
        Self::new(
            root_doc_id(),
            no_parent_doc_id(),
            String::new(), // Empty name for root
        )
    }

    /// Check if this is the root document.
    pub fn is_root(&self) -> bool {
        self.id == root_doc_id()
    }

    /// Get properties as a HashMap (returns a clone).
    /// flutter_rust_bridge:ignore
    pub fn properties_map(&self) -> HashMap<String, Value> {
        self.properties.clone()
    }

    /// Set a property value.
    /// flutter_rust_bridge:ignore
    pub fn set_property(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.properties.insert(key.into(), value.into());
    }

    /// Get a property value.
    /// flutter_rust_bridge:ignore
    pub fn get_property(&self, key: &str) -> Option<Value> {
        self.properties.get(key).cloned()
    }
}

/// Private module for path resolution (hidden from FRB)
mod path_resolution {
    use super::{no_parent_doc_id, root_doc_id, Document};
    use crate::EntityUri;

    /// Trait for stores that can resolve document paths.
    pub trait DocumentPathResolver {
        /// Get a document by ID.
        fn get_document(&self, id: &EntityUri) -> Option<&Document>;
    }

    impl Document {
        /// Derive the filesystem path by walking up the parent chain.
        ///
        /// Returns path segments (e.g., ["projects", "todo"]) which can be
        /// joined with "/" and appended with ".org" for the file path.
        ///
        /// Returns None if any ancestor is missing (orphaned document).
        /// flutter_rust_bridge:ignore
        pub fn derive_path_segments<R: DocumentPathResolver>(
            &self,
            resolver: &R,
        ) -> Option<Vec<String>> {
            if self.is_root() {
                return Some(vec![]);
            }

            let mut segments = vec![self.name.clone()];
            let mut current_parent_id = self.parent_id.clone();

            while current_parent_id != no_parent_doc_id() && current_parent_id != root_doc_id() {
                let parent = resolver.get_document(&current_parent_id)?;
                if !parent.name.is_empty() {
                    segments.push(parent.name.clone());
                }
                current_parent_id = parent.parent_id.clone();
            }

            segments.reverse();
            Some(segments)
        }

        /// Derive the full path as a string (e.g., "projects/todo").
        /// flutter_rust_bridge:ignore
        pub fn derive_path<R: DocumentPathResolver>(&self, resolver: &R) -> Option<String> {
            self.derive_path_segments(resolver)
                .map(|segs| segs.join("/"))
        }
    }
}

// Re-export for crate-internal use only
#[allow(unused_imports)]
pub(crate) use path_resolution::DocumentPathResolver;

/// URI scheme for document references.
/// Format: `holon-doc://{doc_id}` (e.g., `holon-doc://projects/todo.org`)
pub const DOCUMENT_URI_SCHEME: &str = "holon-doc://";

/// Generate a document URI from a document ID.
///
/// Format: `holon-doc://{doc_id}`
pub fn document_uri(doc_id: &str) -> String {
    format!("{}{}", DOCUMENT_URI_SCHEME, doc_id)
}

/// Parse a document URI to extract the document ID.
///
/// Returns Some(doc_id) if the URI is a valid document URI, None otherwise.
///
/// flutter_rust_bridge:ignore
pub fn parse_document_uri(uri: &str) -> Option<&str> {
    uri.strip_prefix(DOCUMENT_URI_SCHEME)
}

/// Check if a string is a document URI.
pub fn is_document_uri(s: &str) -> bool {
    s.starts_with(DOCUMENT_URI_SCHEME)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestResolver {
        docs: HashMap<EntityUri, Document>,
    }

    impl DocumentPathResolver for TestResolver {
        fn get_document(&self, id: &EntityUri) -> Option<&Document> {
            self.docs.get(id)
        }
    }

    #[test]
    fn test_root_document() {
        let root = Document::root();
        assert!(root.is_root());
        assert_eq!(root.parent_id, no_parent_doc_id());
        assert_eq!(root.name, "");
    }

    #[test]
    fn test_derive_path() {
        let mut docs = HashMap::new();

        let root = Document::root();
        docs.insert(root_doc_id(), root);

        let doc1_id = EntityUri::doc("doc-1");
        let projects = Document::new(doc1_id.clone(), root_doc_id(), "projects".to_string());
        docs.insert(doc1_id.clone(), projects);

        let doc2_id = EntityUri::doc("doc-2");
        let todo = Document::new(doc2_id.clone(), doc1_id, "todo".to_string());
        docs.insert(doc2_id, todo.clone());

        let resolver = TestResolver { docs };

        assert_eq!(
            todo.derive_path(&resolver),
            Some("projects/todo".to_string())
        );
    }

    #[test]
    fn test_properties() {
        let mut doc = Document::new(EntityUri::doc("doc-1"), root_doc_id(), "test".to_string());

        doc.set_property("title", Value::from("My Title"));
        doc.set_property("todo_keywords", Value::from("TODO DONE"));

        assert_eq!(doc.get_property("title"), Some(Value::from("My Title")));
    }

    #[test]
    fn test_document_uri() {
        assert_eq!(document_uri("test.org"), "holon-doc://test.org");
        assert!(is_document_uri("holon-doc://test.org"));
        assert!(!is_document_uri("file://test.org"));
        assert_eq!(parse_document_uri("holon-doc://test.org"), Some("test.org"));
    }
}
