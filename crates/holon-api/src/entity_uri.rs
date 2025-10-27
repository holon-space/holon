use fluent_uri::Uri;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::Value;

/// Universal typed identifier for all entities in holon.
///
/// Newtype around `fluent_uri::Uri<String>` — every entity ID is a valid RFC 3986 URI.
///
/// Common schemes:
/// - `doc:uuid` — documents
/// - `block:uuid` — blocks
/// - `sentinel:no_parent` — root parent sentinel
/// - `https://jira.example.com/ISSUE-123` — external entities
///
/// Parsed at system boundaries, carried as a type everywhere.
///
/// flutter_rust_bridge:opaque
#[derive(Clone, Ord, PartialOrd)]
pub struct EntityUri(Uri<String>);

impl EntityUri {
    /// Parse a raw string into an EntityUri. Validates as RFC 3986 URI.
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        let uri = Uri::parse(raw).map_err(|e| anyhow::anyhow!("Invalid URI {raw:?}: {e}"))?;
        Ok(EntityUri(uri.to_owned()))
    }

    /// Parse an owned string into an EntityUri.
    pub fn parse_owned(raw: String) -> anyhow::Result<Self> {
        let uri = Uri::parse(raw).map_err(|e| anyhow::anyhow!("Invalid URI: {e}"))?;
        Ok(EntityUri(uri))
    }

    /// Construct from scheme + opaque path: `"{scheme}:{path}"`.
    pub fn new(scheme: &str, path: &str) -> Self {
        let raw = format!("{scheme}:{path}");
        EntityUri(Uri::parse(raw).unwrap_or_else(|e| {
            panic!("EntityUri::new({scheme:?}, {path:?}) produced invalid URI: {e}")
        }))
    }

    // -- Document constructors --

    pub fn doc(id: &str) -> Self {
        Self::new("doc", id)
    }

    pub fn doc_root() -> Self {
        Self::new("doc", "__root__")
    }

    pub fn doc_random() -> Self {
        Self::new("doc", &uuid::Uuid::new_v4().to_string())
    }

    // -- Block constructors --

    pub fn block(id: &str) -> Self {
        Self::new("block", id)
    }

    pub fn block_random() -> Self {
        Self::new("block", &uuid::Uuid::new_v4().to_string())
    }

    // -- File constructors --
    // File URIs represent on-disk org files (e.g. `file:index.org`, `file:projects/todo.org`).
    // They are transient identifiers used during parsing and resolved to `doc:<uuid>` at startup.

    pub fn file(path: &str) -> Self {
        Self::new("file", path)
    }

    // -- Sentinel --

    pub fn no_parent() -> Self {
        Self::new("sentinel", "no_parent")
    }

    // -- Accessors --

    /// The URI scheme (e.g. "doc", "block", "https").
    pub fn scheme(&self) -> &str {
        self.0.scheme().as_str()
    }

    /// The path component (the entity-specific identifier).
    /// For `doc:my-uuid` this returns `my-uuid`.
    /// For `https://jira.example.com/ISSUE-1` this returns `/ISSUE-1`.
    pub fn id(&self) -> &str {
        self.0.path().as_str()
    }

    /// The full URI as a string slice.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// Access the inner `fluent_uri::Uri<String>`.
    ///
    /// flutter_rust_bridge:ignore
    pub fn inner(&self) -> &Uri<String> {
        &self.0
    }

    pub fn is_doc(&self) -> bool {
        self.scheme() == "doc"
    }

    pub fn is_block(&self) -> bool {
        self.scheme() == "block"
    }

    pub fn is_file(&self) -> bool {
        self.scheme() == "file"
    }

    pub fn is_sentinel(&self) -> bool {
        self.scheme() == "sentinel"
    }

    // -- Aliases for ParentRef compat --

    /// Returns true if this URI refers to a document (either resolved `doc:` or file-based `file:`).
    pub fn is_document(&self) -> bool {
        self.is_doc() || self.is_file()
    }

    /// Alias for `is_sentinel()`.
    pub fn is_no_parent(&self) -> bool {
        self.is_sentinel()
    }

    /// Alias for `as_str()`.
    pub fn as_raw_str(&self) -> &str {
        self.as_str()
    }

    /// Extract the document ID (path component) if this is a doc or file URI.
    /// flutter_rust_bridge:ignore
    pub fn as_document_id(&self) -> Option<&str> {
        if self.is_document() {
            Some(self.id())
        } else {
            None
        }
    }

    /// Extract the block ID (path component) if this is a block URI.
    /// flutter_rust_bridge:ignore
    pub fn as_block_id(&self) -> Option<&str> {
        if self.is_block() {
            Some(self.id())
        } else {
            None
        }
    }

    /// Parse a raw parent_id string into an EntityUri.
    /// Handles `doc:x`, `block:x`, `sentinel:no_parent`, and bare strings (→ `block:x`).
    pub fn from_raw(s: &str) -> Self {
        if let Ok(uri) = Self::parse(s) {
            return uri;
        }
        // Bare string without scheme — treat as block ID
        Self::block(s)
    }

    /// FRB helper: create from string (for Dart FFI boundary).
    pub fn from_string(s: String) -> anyhow::Result<Self> {
        Self::parse_owned(s)
    }

    /// FRB helper: convert to string (for Dart FFI boundary).
    pub fn to_string_repr(&self) -> String {
        self.0.to_string()
    }
}

// -- Trait impls --

impl PartialEq for EntityUri {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for EntityUri {}

impl std::hash::Hash for EntityUri {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.as_str().hash(state)
    }
}

impl fmt::Display for EntityUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for EntityUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "EntityUri({:?})", self.0.as_str())
    }
}

impl Serialize for EntityUri {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EntityUri {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let uri = Uri::<String>::deserialize(deserializer)?;
        Ok(EntityUri(uri))
    }
}

impl From<EntityUri> for String {
    fn from(uri: EntityUri) -> String {
        uri.0.into_string()
    }
}

impl From<EntityUri> for Value {
    fn from(uri: EntityUri) -> Self {
        Value::String(uri.0.into_string())
    }
}

impl TryFrom<Value> for EntityUri {
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        match value {
            Value::String(s) => EntityUri::parse_owned(s)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.to_string().into() }),
            _ => Err("EntityUri requires a string Value".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_opaque() {
        let uri = EntityUri::parse("doc:abc-123").unwrap();
        assert_eq!(uri.scheme(), "doc");
        assert_eq!(uri.id(), "abc-123");
        assert!(uri.is_doc());
        assert!(!uri.is_block());
    }

    #[test]
    fn parse_valid_hierarchical() {
        let uri = EntityUri::parse("https://jira.example.com/ISSUE-1").unwrap();
        assert_eq!(uri.scheme(), "https");
        assert!(!uri.is_doc());
        assert!(!uri.is_block());
    }

    #[test]
    fn parse_invalid() {
        // No scheme → not a valid absolute URI
        assert!(EntityUri::parse("just-a-string").is_err());
    }

    #[test]
    fn constructors() {
        let doc = EntityUri::doc("my-id");
        assert_eq!(doc.as_str(), "doc:my-id");
        assert!(doc.is_doc());

        let block = EntityUri::block("b-1");
        assert_eq!(block.as_str(), "block:b-1");
        assert!(block.is_block());

        let root = EntityUri::doc_root();
        assert_eq!(root.as_str(), "doc:__root__");
        assert_eq!(root.id(), "__root__");

        let np = EntityUri::no_parent();
        assert!(np.is_sentinel());
    }

    #[test]
    fn display() {
        let uri = EntityUri::doc("test");
        assert_eq!(uri.to_string(), "doc:test");
    }

    #[test]
    fn value_round_trip() {
        let uri = EntityUri::block("x");
        let v: Value = uri.clone().into();
        assert_eq!(v, Value::String("block:x".into()));
        let uri2: EntityUri = v.try_into().unwrap();
        assert_eq!(uri, uri2);
    }

    #[test]
    fn serde_round_trip() {
        let uri = EntityUri::doc("abc");
        let json = serde_json::to_string(&uri).unwrap();
        assert_eq!(json, "\"doc:abc\"");
        let parsed: EntityUri = serde_json::from_str(&json).unwrap();
        assert_eq!(uri, parsed);
    }

    #[test]
    fn random_constructors_are_unique() {
        let a = EntityUri::doc_random();
        let b = EntityUri::doc_random();
        assert_ne!(a, b);
    }

    #[test]
    fn full_https_uri() {
        let uri = EntityUri::parse("https://todoist.com/tasks/12345").unwrap();
        assert_eq!(uri.scheme(), "https");
        // For hierarchical URIs, path includes the leading /
        assert_eq!(uri.id(), "/tasks/12345");
    }

    #[test]
    fn parse_uuid_doc_uri() {
        let uri = EntityUri::parse("doc:f3c6fd2d-4784-45b4-9b7c-c05300474ff4").unwrap();
        assert_eq!(uri.scheme(), "doc", "scheme mismatch");
        assert!(uri.is_doc(), "is_doc() should be true for doc:UUID");
    }

    #[test]
    fn equality_and_hash() {
        let a = EntityUri::doc("same");
        let b = EntityUri::doc("same");
        let c = EntityUri::doc("different");
        assert_eq!(a, b);
        assert_ne!(a, c);

        let mut set = std::collections::HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b));
        assert!(!set.contains(&c));
    }
}
