//! Transition arcs (ADR 0031): what an operation READS and what it EMITS.
//!
//! A Petri-net transition is defined by its arcs. An operation that declares
//! none is not simulatable — a second consumer asked to simulate it must
//! REFUSE, never simulate an empty effect. [`TransitionArcs::Undeclared`] is
//! that fail-closed statement, the analogue of
//! `BoundaryBehavior::Unclassified`.
//!
//! Places are PARSED here, from `"relation.field"` into [`ArcPlace`], against a
//! [`SchemaSource`] — the ONE declared field vocabulary (`crate::schema`). This
//! crate is a leaf so `holon-macros` can reach it; `holon-api` re-exports these
//! types as the canonical consumer path.
//!
//! Validation is two-phase, split by BINDING TIME:
//! * [`ArcPlace::parse`] resolves against [`BuiltinSchemas`] and is called at
//!   macro expansion, so a typo in a `#[reads]`/`#[emits]` literal on a
//!   statically declared entity is a compile error.
//! * A relation that exists only at runtime (a created entity type, an MCP
//!   sidecar's) is REPRESENTABLE here — the relation is carried as a name — and
//!   checked by [`TransitionArcs::validate_against`] when the descriptor is
//!   registered.

use std::fmt;

use serde::Deserialize;
use serde::Serialize;

use crate::schema::BuiltinSchemas;
use crate::schema::SchemaSource;

/// Relations that carry authorization state — who may act (`session`) and
/// what they may touch (`membership`). A transition may read them; only the
/// sharing ingress writes them, so no operation descriptor may declare an
/// out-arc or a marking delta into them. The catalog lock
/// (`crates/holon-app/tests/authority_place_reservation.rs`) enforces this
/// over every registered op.
pub const AUTHORITY_RESERVED_RELATIONS: &[&str] = &["membership", "session"];

/// The relation an arc names — an entity type, by name. Open: a relation that
/// exists only at runtime is as representable as `block`, and is checked
/// against its own schema at registration time.
///
/// Deliberately the same vocabulary as [`crate::pattern::Subject`]: an arc
/// names state a guard could also predicate over, so the two must not drift
/// into two dialects.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArcRelation(String);

impl ArcRelation {
    pub fn new(name: impl Into<String>) -> Self {
        ArcRelation(name.into())
    }

    pub fn block() -> Self {
        ArcRelation(crate::schema::block::RELATION.to_string())
    }

    pub fn clock() -> Self {
        ArcRelation(crate::schema::clock::RELATION.to_string())
    }

    /// The wire/source spelling — the same token accepted by
    /// [`ArcPlace::parse`].
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this relation is one of [`AUTHORITY_RESERVED_RELATIONS`].
    pub fn is_authority_reserved(&self) -> bool {
        AUTHORITY_RESERVED_RELATIONS.contains(&self.0.as_str())
    }
}

impl fmt::Display for ArcRelation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Prints the relation NAME, so a refusal quotes what the developer wrote
/// rather than a wrapper type.
impl fmt::Debug for ArcRelation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

/// One `relation.field` cell of state — a Petri-net *place*.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ArcPlace {
    pub relation: ArcRelation,
    pub field: String,
}

impl ArcPlace {
    /// A place on a relation whose schema is not resolvable here — an entity
    /// type that exists only at runtime. Unchecked by construction; the check
    /// is [`TransitionArcs::validate_against`] at registration.
    pub fn new(relation: impl Into<String>, field: impl Into<String>) -> ArcPlace {
        ArcPlace {
            relation: ArcRelation::new(relation),
            field: field.into(),
        }
    }

    /// Parse `"relation.field"` against the in-tree declarations. The macro
    /// calls this at expansion time, so a malformed place is a compile error
    /// pointing at the offending literal.
    pub fn parse(input: &str) -> Result<ArcPlace, ArcParseError> {
        ArcPlace::parse_in(input, &BuiltinSchemas)
    }

    /// Parse against an arbitrary schema source — the same shape check, a
    /// different population of entities.
    pub fn parse_in(input: &str, source: &dyn SchemaSource) -> Result<ArcPlace, ArcParseError> {
        let Some((relation, field)) = input.split_once('.') else {
            return Err(ArcParseError::NotDotted(input.to_string()));
        };
        if field.is_empty() || field.contains('.') || field.contains(char::is_whitespace) {
            return Err(ArcParseError::BadField(field.to_string()));
        }
        let place = ArcPlace::new(relation, field);
        validate_place(&place, source)?;
        Ok(place)
    }
}

/// Check one place against a schema source. Shared by the compile-time parse
/// and the registration-time gate so the two cannot disagree about what a
/// legal place is.
pub fn validate_place(place: &ArcPlace, source: &dyn SchemaSource) -> Result<(), ArcParseError> {
    let Some(known) = source.arc_places(place.relation.as_str()) else {
        return Err(ArcParseError::UnknownRelation {
            relation: place.relation.as_str().to_string(),
            known: source.relations(),
        });
    };
    if !known.contains(&place.field) {
        return Err(ArcParseError::UnknownField {
            relation: place.relation.clone(),
            field: place.field.clone(),
            known,
        });
    }
    Ok(())
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
    /// No schema source knows this relation. The known list travels with the
    /// error so the refusal names what the writer could have meant.
    UnknownRelation {
        relation: String,
        known: Vec<String>,
    },
    BadField(String),
    /// The relation exists but has no such place. A typo'd field would
    /// otherwise be undetectable: containment reds on a MISSING place, never
    /// on one that cannot be written at all.
    UnknownField {
        relation: ArcRelation,
        field: String,
        known: Vec<String>,
    },
}

impl fmt::Display for ArcParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArcParseError::NotDotted(s) => {
                write!(f, "arc place {s:?} is not \"relation.field\"")
            }
            ArcParseError::UnknownRelation { relation, known } => write!(
                f,
                "unknown arc relation {relation:?}; known relations are {known:?}"
            ),
            ArcParseError::BadField(s) => {
                write!(f, "arc place field {s:?} must be one non-empty bare name")
            }
            ArcParseError::UnknownField {
                relation,
                field,
                known,
            } => write!(
                f,
                "relation {relation:?} has no place {field:?}; known places are {known:?}"
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

    /// Every place this declaration names, in and out.
    pub fn places(&self) -> Vec<&ArcPlace> {
        match self {
            TransitionArcs::Undeclared => Vec::new(),
            TransitionArcs::Declared { reads, emits } => reads
                .iter()
                .chain(emits.iter().map(ArcEmit::place))
                .collect(),
        }
    }

    /// The out-arc places naming an authority-reserved relation. Written or
    /// excluded, both are the descriptor claiming a write path into
    /// authorization state — an emit that would mint capabilities.
    pub fn authority_reserved_emits(&self) -> Vec<&ArcPlace> {
        match self {
            TransitionArcs::Undeclared => Vec::new(),
            TransitionArcs::Declared { emits, .. } => emits
                .iter()
                .map(ArcEmit::place)
                .filter(|p| p.relation.is_authority_reserved())
                .collect(),
        }
    }

    /// The registration-time half of the two-phase check: every place must name
    /// a relation the source knows and a field that relation has. A declaration
    /// that arrives from outside the tree (a created entity type, an MCP
    /// sidecar) never passed the macro's compile-time parse, so this is the
    /// only place it is checked.
    pub fn validate_against(&self, source: &dyn SchemaSource) -> Result<(), ArcParseError> {
        for place in self.places() {
            validate_place(place, source)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::BUILTIN_SCHEMAS;

    /// A schema source standing in for an entity type that exists only at
    /// runtime — the population the macro can never see.
    struct RuntimeEntity;

    impl SchemaSource for RuntimeEntity {
        fn arc_places(&self, relation: &str) -> Option<Vec<String>> {
            (relation == "claude_session")
                .then(|| vec!["title".to_string(), "started_at".to_string()])
        }

        fn relations(&self) -> Vec<String> {
            vec!["claude_session".to_string()]
        }
    }

    #[test]
    fn a_place_parses_into_a_typed_relation_and_field() {
        assert_eq!(
            ArcPlace::parse("block.parent_id").expect("parses"),
            ArcPlace {
                relation: ArcRelation::block(),
                field: "parent_id".to_string(),
            }
        );
        assert_eq!(
            ArcPlace::parse("clock.today").expect("parses").relation,
            ArcRelation::clock()
        );
    }

    #[test]
    fn a_malformed_place_is_an_error_naming_what_is_wrong() {
        assert_eq!(
            ArcPlace::parse("parent_id"),
            Err(ArcParseError::NotDotted("parent_id".to_string()))
        );
        assert!(matches!(
            ArcPlace::parse("document.title"),
            Err(ArcParseError::UnknownRelation { relation, .. }) if relation == "document"
        ));
        assert_eq!(
            ArcPlace::parse("block."),
            Err(ArcParseError::BadField(String::new()))
        );
        assert_eq!(
            ArcPlace::parse("block.a.b"),
            Err(ArcParseError::BadField("a.b".to_string()))
        );
    }

    /// The teeth of the declared field list: a typo'd place is rejected.
    /// Without this, `block.totally_bogus_field_xyz` would compile, satisfy
    /// every containment check forever, and never red — containment can only
    /// see a place that is missing, not one that does not exist.
    #[test]
    fn a_field_the_relation_does_not_have_is_rejected() {
        assert!(matches!(
            ArcPlace::parse("block.totally_bogus_field_xyz"),
            Err(ArcParseError::UnknownField { field, .. }) if field == "totally_bogus_field_xyz"
        ));
        // A real block column is not automatically a clock place.
        assert!(matches!(
            ArcPlace::parse("clock.content"),
            Err(ArcParseError::UnknownField { relation, field, .. })
                if relation == ArcRelation::clock() && field == "content"
        ));
        assert!(ArcPlace::parse("clock.today").is_ok());
    }

    /// Storage bookkeeping is not a place. A column exists in the DDL without
    /// being declarable, so the schema's `arc_place` flag has to be the thing
    /// the parser reads — not the column list.
    #[test]
    fn a_column_that_is_not_a_place_is_rejected() {
        for column in ["created_at", "updated_at", "_change_origin", "write_seq"] {
            assert!(
                ArcPlace::parse(&format!("block.{column}")).is_err(),
                "block.{column} is storage bookkeeping, not a declarable place"
            );
        }
    }

    /// Every name the schema advertises must actually parse — a declaration the
    /// parser rejects would be a vocabulary nobody can use.
    #[test]
    fn every_advertised_field_parses() {
        for schema in BUILTIN_SCHEMAS {
            for field in schema.arc_places() {
                let text = format!("{}.{field}", schema.relation);
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
            "unknown arc relation \"document\"; known relations are [\"block\", \"clock\", \"integration\"]"
        );
        assert_eq!(
            msg("block."),
            "arc place field \"\" must be one non-empty bare name"
        );
        let unknown_field = msg("block.totally_bogus_field_xyz");
        assert!(
            unknown_field.contains("has no place \"totally_bogus_field_xyz\"")
                && unknown_field.contains("\"content\""),
            "the refusal must name the place and the vocabulary: {unknown_field}"
        );
    }

    /// A relation the built-ins never heard of is REPRESENTABLE — that is what
    /// makes an MCP-sourced entity's arcs expressible at all — and resolves
    /// against its own source.
    #[test]
    fn a_runtime_relation_is_representable_and_checked_against_its_own_source() {
        let place = ArcPlace::new("claude_session", "title");
        assert!(
            ArcPlace::parse("claude_session.title").is_err(),
            "the built-in source must not admit a runtime relation"
        );
        validate_place(&place, &RuntimeEntity).expect("its own source knows it");
        assert!(matches!(
            validate_place(&ArcPlace::new("claude_session", "nope"), &RuntimeEntity),
            Err(ArcParseError::UnknownField { field, .. }) if field == "nope"
        ));
    }

    /// The registration-time gate walks BOTH directions of the transition: an
    /// unknown place hiding in `reads` must refuse exactly as one in `emits`.
    #[test]
    fn validate_against_refuses_an_unknown_place_on_either_arc() {
        let bad_read = TransitionArcs::Declared {
            reads: vec![ArcPlace::new("claude_session", "nope")],
            emits: vec![],
        };
        let bad_emit = TransitionArcs::Declared {
            reads: vec![],
            emits: vec![ArcEmit::Writes(ArcPlace::new("claude_session", "nope"))],
        };
        for arcs in [bad_read, bad_emit] {
            assert!(arcs.validate_against(&RuntimeEntity).is_err());
        }
        assert!(
            TransitionArcs::Undeclared
                .validate_against(&RuntimeEntity)
                .is_ok(),
            "a refusal to declare names no places, so it cannot name a bad one"
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

    /// Reading authorization state is legal; any out-arc into it — written or
    /// excluded — is reported. The anti-laundering reservation.
    #[test]
    fn an_out_arc_into_an_authority_reserved_relation_is_reported() {
        let reading = TransitionArcs::Declared {
            reads: vec![
                ArcPlace::new("membership", "grantee"),
                ArcPlace::new("session", "principal"),
            ],
            emits: vec![ArcEmit::Writes(
                ArcPlace::parse("block.content").expect("parses"),
            )],
        };
        assert!(reading.authority_reserved_emits().is_empty());

        let minting = TransitionArcs::Declared {
            reads: vec![],
            emits: vec![
                ArcEmit::Writes(ArcPlace::new("membership", "grantee")),
                ArcEmit::Excluded {
                    place: ArcPlace::new("session", "principal"),
                    reason: "written through an unmodeled seam".to_string(),
                },
            ],
        };
        assert_eq!(
            minting
                .authority_reserved_emits()
                .iter()
                .map(|p| p.to_string())
                .collect::<Vec<_>>(),
            vec![
                "membership.grantee".to_string(),
                "session.principal".to_string()
            ]
        );
        assert!(
            TransitionArcs::Undeclared
                .authority_reserved_emits()
                .is_empty()
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
