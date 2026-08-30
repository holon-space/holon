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
    /// Authored advice-suppression exclusion set: the `(anchor, lesson)` pairs
    /// this anchor block has dismissed. Projected to the `advice_suppressed`
    /// junction; serialized as the `:ADVICE_SUPPRESSED:` drawer (ADR 0021).
    AdviceSuppressed,
    /// Compass contribution edges: the blocks this block advances. Projected to
    /// the `block_contributes_to` junction; serialized as the
    /// `:contributes-to:` drawer (docs/Reference/CompassConventions.md).
    ContributesTo,
}

impl EdgeField {
    /// Every edge field. Iterate this — never hand-list `tags`/`requires`.
    pub const ALL: [EdgeField; 4] = [
        EdgeField::Tags,
        EdgeField::Requires,
        EdgeField::AdviceSuppressed,
        EdgeField::ContributesTo,
    ];

    /// The SQL/params column name (and the key used in flattened params).
    pub fn column(self) -> &'static str {
        match self {
            EdgeField::Tags => "tags",
            EdgeField::Requires => "requires",
            EdgeField::AdviceSuppressed => "advice_suppressed",
            EdgeField::ContributesTo => "contributes_to",
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
            EdgeField::AdviceSuppressed => block.advice_suppressed.is_empty(),
            EdgeField::ContributesTo => block.contributes_to.is_empty(),
        }
    }

    /// Whether this edge field differs between two blocks.
    pub fn differs(self, a: &Block, b: &Block) -> bool {
        match self {
            EdgeField::Tags => a.tags != b.tags,
            EdgeField::Requires => a.requires != b.requires,
            EdgeField::AdviceSuppressed => a.advice_suppressed != b.advice_suppressed,
            EdgeField::ContributesTo => a.contributes_to != b.contributes_to,
        }
    }

    /// This edge field's target ids on `block`, mutably. `None` for
    /// [`EdgeField::Tags`], whose members are tag strings, not block references
    /// — so a caller rewriting block ids gets exactly the fields it may touch.
    pub fn targets_mut(self, block: &mut Block) -> Option<&mut Vec<EntityUri>> {
        match self {
            EdgeField::Tags => None,
            EdgeField::Requires => Some(&mut block.requires),
            EdgeField::AdviceSuppressed => Some(&mut block.advice_suppressed),
            EdgeField::ContributesTo => Some(&mut block.contributes_to),
        }
    }

    /// The diff/create param value for this edge field from `block`. Always a
    /// `Value::Array` of id/tag strings — edge fields are set-valued, and the
    /// SQL provider's edge partition routes the Array to the junction table.
    ///
    /// A repeated target is folded out here, keeping the first occurrence.
    /// Every junction keys on `(source, target)`, while three of the four
    /// fields are carried on `Block` as a plain `Vec<EntityUri>` that can
    /// hold the same target twice (`tags` cannot — `Tags` is a `BTreeSet`).
    /// The provider's `edge_field_replace_sql` emits one plain `INSERT` per
    /// element, so a repeat raises a primary-key violation that fails the
    /// whole block write and, under the outbound reconcile's retry, never
    /// converges. This is the one builder every write leg shares, so the
    /// fold belongs here rather than at each call site.
    pub fn param_value(self, block: &Block) -> Value {
        let targets: Vec<String> = match self {
            EdgeField::Tags => block.tags.iter().cloned().collect(),
            EdgeField::Requires => block.requires.iter().map(|r| r.to_string()).collect(),
            EdgeField::AdviceSuppressed => block
                .advice_suppressed
                .iter()
                .map(|r| r.to_string())
                .collect(),
            EdgeField::ContributesTo => {
                block.contributes_to.iter().map(|r| r.to_string()).collect()
            }
        };
        let mut seen = std::collections::HashSet::new();
        Value::Array(
            targets
                .into_iter()
                .filter(|t| seen.insert(t.clone()))
                .map(Value::String)
                .collect(),
        )
    }
}

/// A block's edge fields as ONE value. Every create path carries this instead
/// of one positional argument per field, so adding an edge field is a change
/// here and at [`EdgeField::ALL`] — never a new parameter threaded through
/// call sites that would silently default it away.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlockEdges {
    pub tags: Tags,
    pub requires: Vec<EntityUri>,
    pub advice_suppressed: Vec<EntityUri>,
    pub contributes_to: Vec<EntityUri>,
}

impl BlockEdges {
    /// The edge fields carried by `block`.
    pub fn of(block: &Block) -> Self {
        Self {
            tags: block.tags.clone(),
            requires: block.requires.clone(),
            advice_suppressed: block.advice_suppressed.clone(),
            contributes_to: block.contributes_to.clone(),
        }
    }

    /// Overwrite `block`'s edge fields with these.
    pub fn apply_to(&self, block: &mut Block) {
        block.tags = self.tags.clone();
        block.requires = self.requires.clone();
        block.advice_suppressed = self.advice_suppressed.clone();
        block.contributes_to = self.contributes_to.clone();
    }

    /// Set `field`'s members from the strings a param bag / Loro meta carries:
    /// tag strings for [`EdgeField::Tags`], raw ids for the reference fields
    /// (promoted to `block:`-schemed uris).
    pub fn set_from_raw(&mut self, field: EdgeField, values: Vec<String>) {
        match field {
            EdgeField::Tags => self.tags = values.into_iter().collect(),
            EdgeField::Requires => self.requires = uris_from_raw(values),
            EdgeField::AdviceSuppressed => self.advice_suppressed = uris_from_raw(values),
            EdgeField::ContributesTo => self.contributes_to = uris_from_raw(values),
        }
    }

    /// `field`'s members in the same string shape [`Self::set_from_raw`] takes,
    /// after uri normalization.
    pub fn members(&self, field: EdgeField) -> Vec<String> {
        match field {
            EdgeField::Tags => self.tags.to_vec(),
            EdgeField::Requires => uri_strings(&self.requires),
            EdgeField::AdviceSuppressed => uri_strings(&self.advice_suppressed),
            EdgeField::ContributesTo => uri_strings(&self.contributes_to),
        }
    }

    /// Whether every edge set is empty.
    pub fn is_empty(&self) -> bool {
        self.tags.is_empty()
            && self.requires.is_empty()
            && self.advice_suppressed.is_empty()
            && self.contributes_to.is_empty()
    }
}

fn uris_from_raw(values: Vec<String>) -> Vec<EntityUri> {
    values
        .iter()
        // ALLOW(entity_uri_from_raw): edge targets arriving as param-bag /
        // Loro-meta strings, promoted to schemed uris at this boundary.
        .map(|s| EntityUri::from_raw(s))
        .collect()
}

fn uri_strings(uris: &[EntityUri]) -> Vec<String> {
    uris.iter().map(|u| u.to_string()).collect()
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
    /// Replace the block's `advice_suppressed` exclusion set (the
    /// `advice_suppressed` junction): the `(anchor, lesson)` pairs this anchor
    /// block has dismissed (ADR 0021).
    AdviceSuppressed(Vec<EntityUri>),
    /// Replace the block's `contributes_to` contribution edges (the
    /// `block_contributes_to` junction): the blocks this block advances.
    ContributesTo(Vec<EntityUri>),
}

impl EdgeFieldUpdate {
    /// The edge field this update targets.
    pub fn field(&self) -> EdgeField {
        match self {
            EdgeFieldUpdate::Tags(_) => EdgeField::Tags,
            EdgeFieldUpdate::Requires(_) => EdgeField::Requires,
            EdgeFieldUpdate::AdviceSuppressed(_) => EdgeField::AdviceSuppressed,
            EdgeFieldUpdate::ContributesTo(_) => EdgeField::ContributesTo,
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
        assert_eq!(EdgeField::AdviceSuppressed.column(), "advice_suppressed");
        assert_eq!(EdgeField::ContributesTo.column(), "contributes_to");

        assert!(EdgeField::is_edge_column("tags"));
        assert!(EdgeField::is_edge_column("requires"));
        assert!(EdgeField::is_edge_column("advice_suppressed"));
        assert!(EdgeField::is_edge_column("contributes_to"));
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

    /// Every junction keys on `(source, target)` and the provider emits one
    /// plain `INSERT` per element, so a repeated target fails the whole block
    /// write on the primary key. `Vec<EntityUri>` cannot express that
    /// constraint, so this builder folds the repeat out — keeping the FIRST
    /// occurrence, which holds the surviving targets in authored order.
    ///
    /// Bug-funnel `2026-08-30-edge-field-duplicate-target-wedges-write`.
    #[test]
    fn param_value_folds_a_repeated_target_keeping_authored_order() {
        let b = block(&[], &["late", "early", "late"]);
        assert_eq!(
            EdgeField::Requires.param_value(&b),
            Value::Array(vec![
                Value::String("block:late".to_string()),
                Value::String("block:early".to_string()),
            ]),
            "the repeat is dropped at its SECOND occurrence, so the surviving targets keep the \
             order they were authored in — sorting instead would churn org write-back bytes"
        );
    }
}
