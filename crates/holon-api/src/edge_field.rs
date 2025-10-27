//! Block **edge fields** — the junction-backed, set-valued block attributes
//! (`tags`, `requires`) that are projected to their own tables (`block_tags`,
//! `block_requires`) rather than living as scalar columns on `block_raw`.
//!
//! [`EdgeFieldUpdate`] names the edge-field category as a *type* so a write to
//! one edge field is parameterized over *which* field — neither `tags` nor
//! `requires` is special-cased. [`EdgeField`] is the closed enumeration of the
//! edge fields themselves: any code that projects edge fields iterates
//! [`EdgeField::ALL`] and therefore cannot silently omit one. (That omission
//! was the H12 bug class — `blocks_differ` compared `tags` but not `requires`,
//! so a `requires`-only edit never propagated to the SQL projection.)

use crate::EntityUri;
use crate::Value;
use crate::block::Block;
use crate::types::Tags;

/// The closed set of block edge fields. A write/projection that enumerates edge
/// fields does so over [`EdgeField::ALL`] — adding a new edge field is a single
/// change here that propagates to every projection site, so "handled `tags` but
/// forgot `requires`" (the H12 bug) becomes unrepresentable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeField {
    Tags,
    Requires,
}

impl EdgeField {
    /// Every edge field. Iterate this — never hand-list `tags`/`requires`.
    pub const ALL: [EdgeField; 2] = [EdgeField::Tags, EdgeField::Requires];

    /// The SQL/params column name (and the key used in flattened params).
    pub fn column(self) -> &'static str {
        match self {
            EdgeField::Tags => "tags",
            EdgeField::Requires => "requires",
        }
    }

    /// Whether `key` names an edge field — used by property-flatten guards to
    /// skip edge keys (they belong in junction tables, not the properties
    /// blob).
    pub fn is_edge_column(key: &str) -> bool {
        Self::ALL.iter().any(|f| f.column() == key)
    }

    /// Whether this edge field's set is empty for `block`.
    pub fn is_empty(self, block: &Block) -> bool {
        match self {
            EdgeField::Tags => block.tags.is_empty(),
            EdgeField::Requires => block.requires.is_empty(),
        }
    }

    /// Whether this edge field differs between two blocks.
    pub fn differs(self, a: &Block, b: &Block) -> bool {
        match self {
            EdgeField::Tags => a.tags != b.tags,
            EdgeField::Requires => a.requires != b.requires,
        }
    }

    /// The diff/create param value for this edge field from `block`. Always a
    /// `Value::Array` of id/tag strings — edge fields are set-valued, and the
    /// SQL provider's edge partition routes the Array to the junction table.
    pub fn param_value(self, block: &Block) -> Value {
        match self {
            EdgeField::Tags => Value::Array(
                block
                    .tags
                    .iter()
                    .map(|t| Value::String(t.clone()))
                    .collect(),
            ),
            EdgeField::Requires => Value::Array(
                block
                    .requires
                    .iter()
                    .map(|r| Value::String(r.to_string()))
                    .collect(),
            ),
        }
    }
}

/// An update to exactly one of a block's edge fields. Carried as a typed value
/// (not a stringly-typed map) so the boundary parse happens once, at
/// construction, per the parse-don't-validate rule.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EdgeFieldUpdate {
    /// Replace the block's `tags` set (the `block_tags` junction).
    Tags(Tags),
    /// Replace the block's `requires` dependency edges (the `block_requires`
    /// junction): the block ids this block depends on / is blocked by.
    Requires(Vec<EntityUri>),
}

impl EdgeFieldUpdate {
    /// The edge field this update targets.
    pub fn field(&self) -> EdgeField {
        match self {
            EdgeFieldUpdate::Tags(_) => EdgeField::Tags,
            EdgeFieldUpdate::Requires(_) => EdgeField::Requires,
        }
    }

    /// The edge field name this update targets, for labelling / display.
    pub fn field_name(&self) -> &'static str {
        self.field().column()
    }
}

#[cfg(test)]
mod mutation_gap_tests {
    use super::*;

    fn block(tags: &[&str], requires: &[&str]) -> Block {
        let mut b = Block::new_text(
            EntityUri::parse_owned("block:b1".to_string()).unwrap(),
            EntityUri::no_parent(),
            "x",
        );
        b.tags = Tags::from(tags.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        b.requires = requires
            .iter()
            .map(|s| EntityUri::parse_owned(format!("block:{s}")).unwrap())
            .collect();
        b
    }

    #[test]
    fn edge_field_closed_enum_surface() {
        assert_eq!(EdgeField::Tags.column(), "tags");
        assert_eq!(EdgeField::Requires.column(), "requires");

        assert!(EdgeField::is_edge_column("tags"));
        assert!(EdgeField::is_edge_column("requires"));
        assert!(!EdgeField::is_edge_column("content"));
        assert!(!EdgeField::is_edge_column("tag"));

        let empty = block(&[], &[]);
        let tagged = block(&["a"], &[]);
        let requiring = block(&[], &["dep"]);

        assert!(EdgeField::Tags.is_empty(&empty));
        assert!(!EdgeField::Tags.is_empty(&tagged));
        assert!(EdgeField::Requires.is_empty(&tagged));
        assert!(!EdgeField::Requires.is_empty(&requiring));

        assert!(EdgeField::Tags.differs(&empty, &tagged));
        assert!(!EdgeField::Tags.differs(&empty, &requiring));
        assert!(EdgeField::Requires.differs(&empty, &requiring));
        assert!(!EdgeField::Requires.differs(&empty, &tagged));

        assert_eq!(
            EdgeField::Tags.param_value(&tagged),
            Value::Array(vec![Value::String("a".to_string())])
        );
        assert_eq!(
            EdgeField::Requires.param_value(&requiring),
            Value::Array(vec![Value::String("block:dep".to_string())])
        );
        assert_eq!(EdgeField::Tags.param_value(&empty), Value::Array(vec![]));

        assert_eq!(
            EdgeFieldUpdate::Tags(Tags::default()).field(),
            EdgeField::Tags
        );
        assert_eq!(
            EdgeFieldUpdate::Requires(vec![]).field(),
            EdgeField::Requires
        );
        assert_eq!(EdgeFieldUpdate::Tags(Tags::default()).field_name(), "tags");
        assert_eq!(EdgeFieldUpdate::Requires(vec![]).field_name(), "requires");
    }
}
