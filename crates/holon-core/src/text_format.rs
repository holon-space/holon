//! What a text format must provide before a VCS over it can be a
//! [`Consolidator`](crate::consolidator::Consolidator).
//!
//! The lift from a file change to an entity delta is parse-and-diff: parse the
//! old bytes and the new bytes, key both by `EntityUri`, diff the two sets.
//! That is what the org ingest already does, so a VCS consolidator adds no
//! second diff engine and the format owes no line spans.

use holon_api::EntityUri;

/// The format cannot carry a stable id in its bytes.
///
/// A format that returns this can feed creates but can never assert a delete,
/// which makes it an import source rather than a replica.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported;

/// One entity parsed out of a file, with the containment the parser found.
pub trait ParsedEntity {
    fn kind(&self) -> &str;
}

/// Bytes ↔ entities, for a format whose files can act as a replica.
pub trait TextFormat: Send + Sync {
    type Entity: ParsedEntity;
    type Tree;
    type ParseError: std::fmt::Display;

    /// Deterministic and total over the format. A file that does not parse is
    /// a refusal carrying a location, never a partial tree.
    fn parse(&self, bytes: &[u8]) -> std::result::Result<Self::Tree, Self::ParseError>;

    /// Round-trip law: `render(parse(b)) == b`, byte for byte, for every file
    /// the app has not changed. A lossy renderer makes every write-back look
    /// like an external edit to the base diff.
    fn render(&self, tree: &Self::Tree) -> Vec<u8>;

    /// The stable id carried in the bytes, if the format can carry one here.
    fn entity_id(&self, entity: &Self::Entity) -> Option<EntityUri>;

    /// Write a minted id into the entity's bytes, so it is in VCS history from
    /// then on.
    fn stamp_id(
        &self,
        entity: &mut Self::Entity,
        id: EntityUri,
    ) -> std::result::Result<(), Unsupported>;

    /// Which fields this format can carry for `kind`.
    fn carries(&self, kind: &str, field: &str) -> bool;

    /// A file holding VCS conflict markers parses as no diff. Without this one
    /// guard a conflicted file reads as a delete of everything in it.
    fn is_conflicted(&self, bytes: &[u8]) -> bool;
}

/// A VCS over a [`TextFormat`], as the second consolidator (ruling D179.d).
/// It implements no [`Consolidator`].
pub struct GitConsolidator<F: TextFormat> {
    pub format: F,
}

impl<F: TextFormat> GitConsolidator<F> {
    pub fn new(format: F) -> Self {
        Self { format }
    }
}
