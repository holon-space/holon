//! Transition arcs (ADR 0031): what an operation READS and what it EMITS.
//!
//! A Petri-net transition is defined by its arcs. An operation that declares
//! none is not simulatable — a second consumer asked to simulate it must
//! REFUSE, never simulate an empty effect. [`TransitionArcs::Undeclared`] is
//! that fail-closed statement, the analogue of
//! `BoundaryBehavior::Unclassified`.
//!
//! Places are PARSED here, at macro-expansion time, from `"relation.field"`
//! into [`ArcPlace`] — so an unknown relation is a compile error rather than a
//! string that nobody ever checks. This crate is a leaf so `holon-macros` can
//! reach it; `holon-api` re-exports these types as the canonical consumer path.

use std::fmt;

use serde::Deserialize;
use serde::Serialize;

/// The relations an arc may name. Deliberately closed and deliberately the
/// same vocabulary as [`crate::pattern::Subject`]: an arc names state a guard
/// could also predicate over, so the two must not drift into two dialects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArcRelation {
    Block,
    Clock,
}

/// Every place name the `block` relation admits. Closed, so a typo'd field is
/// a compile error rather than a declaration that is silently true forever —
/// containment can only catch a place that is MISSING, never one that does not
/// exist.
///
/// Three groups, and the split carries the meaning:
/// 1. Intent-writable columns — the vocabulary `BlockWriteField::parse`
///    (`holon-api/src/block_write_field.rs`) admits. The two lists are locked
///    together by `intent_writable_fields_are_all_arc_places` in
///    `holon-api/tests/descriptor_arcs_roundtrip.rs`; this crate is a leaf and
///    cannot import that enum, so the lock lives one crate up.
/// 2. Junction-backed edge sets, written through the edge-field writers.
/// 3. Places namable only to be READ or EXCLUDED. `id`, `parent_id`,
///    `properties` and `content` are what `ProjectionSchema`
///    (`holon/src/api/guard_world.rs`) compiles guards against; the order keys
///    exist here so an op can declare them excluded and for no other reason.
const BLOCK_FIELDS: &[&str] = &[
    // 1. intent-writable columns
    "content",
    "content_type",
    "source_language",
    "source_name",
    "marks",
    "collapsed",
    "widget_only",
    "completed",
    "block_type",
    "properties",
    "tags",
    "task_state",
    "parent_id",
    // 2. junction-backed edge sets
    "requires",
    "advice_suppressed",
    // 3. read-only / excludable
    "id",
    "sort_key",
    "after_block_id",
];

/// The clock relation's places. `today` is the column `ProjectionSchema`'s
/// `clock_relation` selects; `grain` is the column that selects the day row.
const CLOCK_FIELDS: &[&str] = &["today", "grain"];

impl ArcRelation {
    /// The wire/source spelling — the same token accepted by
    /// [`ArcPlace::parse`].
    pub fn as_str(self) -> &'static str {
        match self {
            ArcRelation::Block => "block",
            ArcRelation::Clock => "clock",
        }
    }

    /// Every field this relation admits — the parser's vocabulary, and the
    /// list the drift locks compare against their siblings.
    pub fn known_fields(self) -> &'static [&'static str] {
        match self {
            ArcRelation::Block => BLOCK_FIELDS,
            ArcRelation::Clock => CLOCK_FIELDS,
        }
    }
}

impl fmt::Display for ArcRelation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One `relation.field` cell of state — a Petri-net *place*.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ArcPlace {
    pub relation: ArcRelation,
    pub field: String,
}

impl ArcPlace {
    /// Parse `"relation.field"`. The macro calls this at expansion time, so a
    /// malformed place is a compile error pointing at the offending literal.
    pub fn parse(input: &str) -> Result<ArcPlace, ArcParseError> {
        let Some((relation, field)) = input.split_once('.') else {
            return Err(ArcParseError::NotDotted(input.to_string()));
        };
        let relation = match relation {
            "block" => ArcRelation::Block,
            "clock" => ArcRelation::Clock,
            other => return Err(ArcParseError::UnknownRelation(other.to_string())),
        };
        if field.is_empty() || field.contains('.') || field.contains(char::is_whitespace) {
            return Err(ArcParseError::BadField(field.to_string()));
        }
        if !relation.known_fields().contains(&field) {
            return Err(ArcParseError::UnknownField {
                relation,
                field: field.to_string(),
            });
        }
        Ok(ArcPlace {
            relation,
            field: field.to_string(),
        })
    }
}

impl fmt::Display for ArcPlace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.relation, self.field)
    }
}

/// Why a place string is not a place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArcParseError {
    NotDotted(String),
    UnknownRelation(String),
    BadField(String),
    /// The relation exists but has no such place. A typo'd field would
    /// otherwise be undetectable: containment reds on a MISSING place, never
    /// on one that cannot be written at all.
    UnknownField {
        relation: ArcRelation,
        field: String,
    },
}

impl fmt::Display for ArcParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArcParseError::NotDotted(s) => {
                write!(f, "arc place {s:?} is not \"relation.field\"")
            }
            ArcParseError::UnknownRelation(r) => write!(
                f,
                "unknown arc relation {r:?}; known relations are \"block\" and \"clock\""
            ),
            ArcParseError::BadField(s) => {
                write!(f, "arc place field {s:?} must be one non-empty bare name")
            }
            ArcParseError::UnknownField { relation, field } => write!(
                f,
                "relation {relation:?} has no place {field:?}; known places are {:?}",
                relation.known_fields()
            ),
        }
    }
}

impl std::error::Error for ArcParseError {}

/// One out-arc of a transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArcEmit {
    /// The op may write this place.
    Writes(ArcPlace),
    /// ADR 0031 "declarable-as-EXCLUDED": deliberately below the declaration
    /// boundary — another authority owns this place, or the op reaches it only
    /// through a seam that is not modelled. Silence about a written place is a
    /// red; this is the only way to be quiet about one, and it costs a reason.
    Excluded { place: ArcPlace, reason: String },
}

impl ArcEmit {
    /// The place this arc names, written or excluded.
    pub fn place(&self) -> &ArcPlace {
        match self {
            ArcEmit::Writes(place) => place,
            ArcEmit::Excluded { place, .. } => place,
        }
    }
}

/// An operation's declared read/write arcs. Non-defaultable, following the
/// [`crate::pattern::OpGuard`] house pattern: the macro emits
/// [`TransitionArcs::Undeclared`] when no `#[reads]`/`#[emits]` is present, and
/// `Undeclared` is a stated fact ("not simulatable") rather than an absence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TransitionArcs {
    /// Fail-closed. A consumer asked to simulate this op REFUSES loudly.
    Undeclared,
    Declared {
        reads: Vec<ArcPlace>,
        emits: Vec<ArcEmit>,
    },
}

impl TransitionArcs {
    /// The declared out-arcs, or `None` when the op is undeclared. `None` and
    /// `Some(&[])` mean different things: "cannot say" vs "writes nothing".
    pub fn emits(&self) -> Option<&[ArcEmit]> {
        match self {
            TransitionArcs::Undeclared => None,
            TransitionArcs::Declared { emits, .. } => Some(emits),
        }
    }

    /// The declared in-arcs, or `None` when the op is undeclared.
    pub fn reads(&self) -> Option<&[ArcPlace]> {
        match self {
            TransitionArcs::Undeclared => None,
            TransitionArcs::Declared { reads, .. } => Some(reads),
        }
    }

    /// The places the op declares it MAY write — excluded places omitted. This
    /// is the static over-approximation an oracle compares observed writes
    /// against.
    pub fn written_places(&self) -> Vec<&ArcPlace> {
        match self {
            TransitionArcs::Undeclared => Vec::new(),
            TransitionArcs::Declared { emits, .. } => emits
                .iter()
                .filter_map(|e| match e {
                    ArcEmit::Writes(place) => Some(place),
                    ArcEmit::Excluded { .. } => None,
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_place_parses_into_a_typed_relation_and_field() {
        assert_eq!(
            ArcPlace::parse("block.parent_id").expect("parses"),
            ArcPlace {
                relation: ArcRelation::Block,
                field: "parent_id".to_string(),
            }
        );
        assert_eq!(
            ArcPlace::parse("clock.today").expect("parses").relation,
            ArcRelation::Clock
        );
    }

    #[test]
    fn a_malformed_place_is_an_error_naming_what_is_wrong() {
        assert_eq!(
            ArcPlace::parse("parent_id"),
            Err(ArcParseError::NotDotted("parent_id".to_string()))
        );
        assert_eq!(
            ArcPlace::parse("document.title"),
            Err(ArcParseError::UnknownRelation("document".to_string()))
        );
        assert_eq!(
            ArcPlace::parse("block."),
            Err(ArcParseError::BadField(String::new()))
        );
        assert_eq!(
            ArcPlace::parse("block.a.b"),
            Err(ArcParseError::BadField("a.b".to_string()))
        );
    }

    /// The teeth of the closed field list: a typo'd place is rejected. Without
    /// this, `block.totally_bogus_field_xyz` would compile, satisfy every
    /// containment check forever, and never red — containment can only see a
    /// place that is missing, not one that does not exist.
    #[test]
    fn a_field_the_relation_does_not_have_is_rejected() {
        assert_eq!(
            ArcPlace::parse("block.totally_bogus_field_xyz"),
            Err(ArcParseError::UnknownField {
                relation: ArcRelation::Block,
                field: "totally_bogus_field_xyz".to_string(),
            })
        );
        // A real block column is not automatically a clock place.
        assert_eq!(
            ArcPlace::parse("clock.content"),
            Err(ArcParseError::UnknownField {
                relation: ArcRelation::Clock,
                field: "content".to_string(),
            })
        );
        assert!(ArcPlace::parse("clock.today").is_ok());
    }

    /// Every name the relation advertises must actually parse — a list entry
    /// the parser rejects would be a vocabulary nobody can use.
    #[test]
    fn every_advertised_field_parses() {
        for relation in [ArcRelation::Block, ArcRelation::Clock] {
            for field in relation.known_fields() {
                let text = format!("{relation}.{field}");
                let place = ArcPlace::parse(&text)
                    .unwrap_or_else(|e| panic!("advertised place {text} must parse: {e}"));
                assert_eq!(place.to_string(), text);
            }
        }
    }

    /// The parse errors are diagnostics a developer reads at a compile error,
    /// so their text is part of the contract, not an implementation detail.
    #[test]
    fn a_parse_error_says_what_is_wrong_and_what_is_allowed() {
        let msg = |s: &str| ArcPlace::parse(s).expect_err("rejected").to_string();
        assert_eq!(
            msg("parent_id"),
            "arc place \"parent_id\" is not \"relation.field\""
        );
        assert_eq!(
            msg("document.title"),
            "unknown arc relation \"document\"; known relations are \"block\" and \"clock\""
        );
        assert_eq!(
            msg("block."),
            "arc place field \"\" must be one non-empty bare name"
        );
    }

    /// The in-arcs are readable, and `Undeclared` refuses to answer for them
    /// exactly as it does for the out-arcs.
    #[test]
    fn reads_are_readable_and_undeclared_refuses() {
        let arcs = TransitionArcs::Declared {
            reads: vec![ArcPlace::parse("clock.today").expect("parses")],
            emits: vec![],
        };
        assert_eq!(
            arcs.reads(),
            Some(&[ArcPlace::parse("clock.today").expect("parses")][..])
        );
        assert_eq!(TransitionArcs::Undeclared.reads(), None);
    }

    /// An out-arc names its place whether it is written or excluded — the
    /// accessor a consumer uses to enumerate everything an op touches.
    #[test]
    fn an_out_arc_names_its_place_written_or_excluded() {
        let place = ArcPlace::parse("block.sort_key").expect("parses");
        assert_eq!(ArcEmit::Writes(place.clone()).place(), &place);
        assert_eq!(
            ArcEmit::Excluded {
                place: place.clone(),
                reason: "the ordering authority mints order keys".to_string(),
            }
            .place(),
            &place
        );
    }

    /// `Undeclared` says "cannot say"; a declared-but-empty emits says "writes
    /// nothing". Collapsing them would make silence indistinguishable from a
    /// stated absence, which is the whole point of the fail-closed variant.
    #[test]
    fn undeclared_and_declared_empty_are_distinguishable() {
        assert_eq!(TransitionArcs::Undeclared.emits(), None);
        assert_eq!(
            TransitionArcs::Declared {
                reads: vec![],
                emits: vec![],
            }
            .emits(),
            Some(&[][..])
        );
    }

    #[test]
    fn written_places_omit_the_excluded_ones() {
        let arcs = TransitionArcs::Declared {
            reads: vec![ArcPlace::parse("block.content").expect("parses")],
            emits: vec![
                ArcEmit::Writes(ArcPlace::parse("block.content").expect("parses")),
                ArcEmit::Excluded {
                    place: ArcPlace::parse("block.sort_key").expect("parses"),
                    reason: "order keys are minted by the ordering authority".to_string(),
                },
            ],
        };
        assert_eq!(
            arcs.written_places()
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>(),
            vec!["block.content".to_string()]
        );
    }
}
