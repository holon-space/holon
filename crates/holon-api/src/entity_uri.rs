use fluent_uri::Uri;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::Value;

/// Universal typed identifier for all entities in holon.
///
/// Newtype around `fluent_uri::Uri<String>` — every entity ID is a valid RFC 3986 URI.
///
/// Common schemes:
/// - `block:uuid` — blocks (pages are blocks tagged `Page`)
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
        // Guard against double-scheming: the `path` must be a bare value, not an
        // already-scheme-qualified URI. Passing e.g. `EntityUri::block("block:abc")`
        // would silently produce `"block:block:abc"`, and a downstream
        // `UPDATE … WHERE id = 'block:block:abc'` then matches zero rows — the
        // class of bug behind KF-8 (a dropped re-parent in SqlOnly). Restricted
        // to KNOWN holon schemes so synthetic ids (`default-main-panel::src::0`)
        // and `block:{peer}:{counter}` paths don't false-positive. For an
        // already-schemed string use [`EntityUri::from_raw`] / [`EntityUri::parse`].
        debug_assert!(
            !["block:", "file:", "sentinel:"]
                .iter()
                .any(|p| path.starts_with(p)),
            "EntityUri::new({scheme:?}, {path:?}): path already begins with a scheme \
             prefix — double-scheme bug. Use EntityUri::from_raw / parse for an \
             already-schemed string, or pass the bare id."
        );
        let raw = format!("{scheme}:{path}");
        EntityUri(Uri::parse(raw).unwrap_or_else(|e| {
            panic!("EntityUri::new({scheme:?}, {path:?}) produced invalid URI: {e}")
        }))
    }

    // -- Block constructors --

    pub fn block(id: &str) -> Self {
        // `id` must be a BARE block id (a UUID or Loro stable id), never an
        // already-schemed URI. Passing `"block:abc"` here mints
        // `"block:block:abc"` — the recurring re-scheme bug. Use
        // [`EntityUri::from_raw`] (idempotent) when the input may already
        // carry a scheme.
        debug_assert!(
            !id.starts_with("block:"),
            "EntityUri::block() got an already-schemed id {id:?}; use EntityUri::parse() instead"
        );
        Self::new("block", id)
    }

    pub fn block_random() -> Self {
        Self::new("block", &uuid::Uuid::new_v4().to_string())
    }

    /// Create a block URI from a LoroTree TreeID: `block:{peer}:{counter}`
    pub fn block_from_tree_id(peer: u64, counter: i32) -> Self {
        Self::new("block", &format!("{peer}:{counter}"))
    }

    /// Parse a block URI back to LoroTree TreeID components.
    /// Returns `(peer, counter)` if this is a `block:{peer}:{counter}` URI.
    pub fn to_tree_id_parts(&self) -> Option<(u64, i32)> {
        if !self.is_block() {
            return None;
        }
        let id = self.id();
        let (peer_str, counter_str) = id.split_once(':')?;
        let peer: u64 = peer_str.parse().ok()?; // ALLOW(ok): boundary parse for TreeID
        let counter: i32 = counter_str.parse().ok()?; // ALLOW(ok): boundary parse for TreeID
        Some((peer, counter))
    }

    // -- File constructors --
    // File URIs represent on-disk org files (e.g. `file:index.org`, `file:projects/todo.org`).
    // They are transient identifiers used during parsing and resolved to the page's `block:<uuid>` at startup.

    pub fn file(path: &str) -> Self {
        use fluent_uri::encoding::{
            encoder::{Data, Path},
            EString,
        };
        let mut buf = EString::<Path>::new();
        for (i, segment) in path.split('/').enumerate() {
            if i > 0 {
                buf.push('/');
            }
            buf.encode::<Data>(segment);
        }
        Self::new("file", &buf.into_string())
    }

    // -- Sentinel --

    pub fn no_parent() -> Self {
        Self::new("sentinel", "no_parent")
    }

    // -- Accessors --

    /// The URI scheme (e.g. "block", "file", "https").
    pub fn scheme(&self) -> &str {
        self.0.scheme().as_str()
    }

    /// The path component (the entity-specific identifier).
    /// For `block:my-uuid` this returns `my-uuid`.
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

    pub fn is_block(&self) -> bool {
        self.scheme() == "block"
    }

    pub fn is_file(&self) -> bool {
        self.scheme() == "file"
    }

    pub fn is_sentinel(&self) -> bool {
        self.scheme() == "sentinel"
    }

    /// Alias for `is_sentinel()`.
    pub fn is_no_parent(&self) -> bool {
        self.is_sentinel()
    }

    /// Alias for `as_str()`.
    pub fn as_raw_str(&self) -> &str {
        self.as_str()
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
    /// Handles `block:x`, `sentinel:no_parent`, and bare strings (→ `block:x`).
    ///
    /// **Boundary-only (parse, don't validate).** Call this *once*, at the edge
    /// where a string first enters the system from something we don't control —
    /// the org parser, a SQL/matview row, a Loro field, an MCP/FFI param. From
    /// there on, pass the `EntityUri` through; do not turn it back into a string
    /// and re-`from_raw` it deeper in. Re-parsing internal data is the smell
    /// that lets scheme-fragility (bare `x` vs `block:x`) leak between layers —
    /// it's flagged by archlint `entity_uri_from_raw`. When the scheme is known
    /// statically prefer `EntityUri::block(..)` / `EntityUri::parse(..)`.
    pub fn from_raw(s: &str) -> Self {
        if let Ok(uri) = Self::parse(s) {
            // A bare synthetic id containing `::` separators (e.g.
            // `root-layout::src::0`, `default-main-panel::render::0`) is a
            // valid RFC 3986 URI: scheme `root-layout`, path `:src::0`. A real
            // `scheme:path` URI (other than the entity schemes below) never has
            // a path starting with `:` — that shape is the head of a bare id,
            // not a scheme. Mis-accepting it made `is_block()` false downstream,
            // so `resolve_to_tree_id` silently missed existing Loro nodes and
            // field writes forked to the SQL path (layout-swap Loro divergence,
            // 2026-06-11).
            //
            // EXCEPTION: the entity scheme `block` DOES legitimately carry a
            // colon-leading id — the reference model's synthetic split ids are
            // `block::split-N` (scheme `block`, id `:split-N`). Those must
            // round-trip (e.g. `MemoryBackend::children_of` re-parses stored
            // child ids), so accept the block scheme even with a `:`-leading
            // id. Bare layout ids keep their non-entity scheme and stay bare.
            if !uri.id().starts_with(':') || uri.scheme() == "block" {
                return uri;
            }
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
        let uri = EntityUri::parse("block:abc-123").unwrap();
        assert_eq!(uri.scheme(), "block");
        assert_eq!(uri.id(), "abc-123");
        assert!(uri.is_block());
    }

    #[test]
    fn parse_valid_hierarchical() {
        let uri = EntityUri::parse("https://jira.example.com/ISSUE-1").unwrap();
        assert_eq!(uri.scheme(), "https");
        assert!(!uri.is_sentinel());
        assert!(!uri.is_block());
    }

    #[test]
    fn parse_invalid() {
        // No scheme → not a valid absolute URI
        assert!(EntityUri::parse("just-a-string").is_err());
    }

    /// Bare synthetic ids with `::` separators must parse as BLOCK ids, not
    /// as a URI with the id's head for a scheme. `root-layout::src::0` is a
    /// valid RFC 3986 URI (scheme `root-layout`, path `:src::0`) — accepting
    /// that parse made `is_block()` false and `resolve_to_tree_id` miss
    /// existing Loro nodes (layout-swap Loro divergence, 2026-06-11).
    #[test]
    fn from_raw_double_colon_synthetic_id_is_block() {
        for raw in [
            "root-layout::src::0",
            "default-main-panel::render::0",
            "block:root-layout::src::0",
        ] {
            // ALLOW(entity_uri_from_raw): test exercises from_raw parsing directly
            let uri = EntityUri::from_raw(raw);
            assert!(uri.is_block(), "{raw:?} → {uri:?} must be a block URI");
            assert_eq!(uri.id(), raw.strip_prefix("block:").unwrap_or(raw));
            // Idempotent: re-parsing the schemed form round-trips.
            // ALLOW(entity_uri_from_raw): test exercises from_raw parsing directly
            let again = EntityUri::from_raw(uri.as_str());
            assert_eq!(again, uri);
        }
        // Real schemes are untouched.
        // ALLOW(entity_uri_from_raw): test exercises from_raw parsing directly
        assert_eq!(EntityUri::from_raw("person:abc").scheme(), "person");
        // ALLOW(entity_uri_from_raw): test exercises from_raw parsing directly
        assert!(EntityUri::from_raw("sentinel:no_parent").is_sentinel());
        assert_eq!(
            // ALLOW(entity_uri_from_raw): test exercises from_raw parsing directly
            EntityUri::from_raw("https://jira.example.com/ISSUE-1").scheme(),
            "https"
        );
    }

    #[test]
    fn constructors() {
        let block = EntityUri::block("my-id");
        assert_eq!(block.as_str(), "block:my-id");
        assert!(block.is_block());

        let block2 = EntityUri::block("b-1");
        assert_eq!(block2.as_str(), "block:b-1");
        assert!(block2.is_block());

        let np = EntityUri::no_parent();
        assert_eq!(np.as_str(), "sentinel:no_parent");
        assert!(np.is_sentinel());
    }

    #[test]
    fn display() {
        let uri = EntityUri::block("test");
        assert_eq!(uri.to_string(), "block:test");
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
        let uri = EntityUri::block("abc");
        let json = serde_json::to_string(&uri).unwrap();
        assert_eq!(json, "\"block:abc\"");
        let parsed: EntityUri = serde_json::from_str(&json).unwrap();
        assert_eq!(uri, parsed);
    }

    #[test]
    fn random_constructors_are_unique() {
        let a = EntityUri::block_random();
        let b = EntityUri::block_random();
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
    fn parse_uuid_block_uri() {
        let uri = EntityUri::parse("block:f3c6fd2d-4784-45b4-9b7c-c05300474ff4").unwrap();
        assert_eq!(uri.scheme(), "block");
        assert!(uri.is_block());
    }

    #[test]
    fn equality_and_hash() {
        let a = EntityUri::block("same");
        let b = EntityUri::block("same");
        let c = EntityUri::block("different");
        assert_eq!(a, b);
        assert_ne!(a, c);

        let mut set = std::collections::HashSet::new();
        set.insert(a.clone());
        assert!(set.contains(&b));
        assert!(!set.contains(&c));
    }

    #[test]
    fn file_uri_with_spaces_is_percent_encoded() {
        let uri = EntityUri::file("Projects/Holon/Entity Identity.org");
        assert_eq!(uri.as_str(), "file:Projects/Holon/Entity%20Identity.org");
        assert!(uri.is_file());
        assert_eq!(uri.id(), "Projects/Holon/Entity%20Identity.org");
    }

    #[test]
    fn file_uri_with_special_chars_is_percent_encoded() {
        let uri = EntityUri::file("path/to/file#1.org");
        assert_eq!(uri.as_str(), "file:path/to/file%231.org");
    }

    #[test]
    fn file_uri_absolute_path() {
        let uri = EntityUri::file("/absolute/path/file.org");
        assert_eq!(uri.as_str(), "file:/absolute/path/file.org");
    }

    #[test]
    fn file_uri_preserves_hyphens_and_dots() {
        let uri = EntityUri::file("my-dir/sub_dir/file.name-v1.org");
        assert_eq!(uri.as_str(), "file:my-dir/sub_dir/file.name-v1.org");
    }

    #[test]
    fn file_uri_round_trips_through_parse() {
        let uri = EntityUri::file("Projects/Holon/Entity Identity.org");
        let s = uri.to_string();
        let parsed = EntityUri::parse(&s).unwrap();
        assert_eq!(uri, parsed);
        assert!(parsed.is_file());
    }
}
