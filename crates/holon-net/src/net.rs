//! The compiled net's vocabulary. Place identity is `relation.field`
//! ([`ArcPlace`]), shared verbatim with the declaration surface so the two
//! cannot drift.

use std::collections::BTreeSet;

use holon_pattern::Value;
use holon_pattern::arcs::ArcPlace;
use holon_pattern::arcs::ArcRelation;
use holon_pattern::pattern::CmpOp;
use holon_pattern::pattern::Pattern;
use holon_pattern::schema::block;
use serde::Deserialize;
use serde::Serialize;

use crate::bridge::TransitionKey;
use crate::bridge::TransitionSource;

/// Names the token a [`CorrelatedGroup`]'s arcs are all tested against — the
/// CPN-orthodox join of ADR 0032 §2, where one transition's arcs unify a
/// variable across entities.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BindingVar(pub String);

impl BindingVar {
    pub fn new(name: impl Into<String>) -> Self {
        BindingVar(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which way one hop step crosses the relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HopDirection {
    /// The entity the subject's `from` cell points at: `other.to == cur.from`.
    /// `parent(…)` is the forward hop.
    Forward,
    /// The entities pointing back at the subject: `other.from == cur.to`.
    /// `child(…)` is the inverse hop.
    Inverse,
}

/// One step of a correlation.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Hop {
    pub from: ArcPlace,
    pub to: ArcPlace,
    pub direction: HopDirection,
}

impl Hop {
    /// The subject's parent: `block.parent_id` forward onto `block.id`.
    pub fn parent() -> Hop {
        Hop {
            from: ArcPlace::new(block::RELATION, block::PARENT_ID),
            to: ArcPlace::new(block::RELATION, block::ID),
            direction: HopDirection::Forward,
        }
    }

    /// The subject's children: the inverse of [`Hop::parent`].
    pub fn child() -> Hop {
        Hop {
            from: ArcPlace::new(block::RELATION, block::PARENT_ID),
            to: ArcPlace::new(block::RELATION, block::ID),
            direction: HopDirection::Inverse,
        }
    }

    /// The blocks sharing the subject's parent, keyed on `parent_id` itself.
    ///
    /// One step, not `parent` then `child`: the composition needs the parent
    /// to be a ROW and so reaches nothing from a root block, while the
    /// profile's `rule_sibling(parent_id)` lookup treats two roots as
    /// siblings. Keying on the column reproduces the lookup.
    pub fn sibling() -> Hop {
        Hop {
            from: ArcPlace::new(block::RELATION, block::PARENT_ID),
            to: ArcPlace::new(block::RELATION, block::PARENT_ID),
            direction: HopDirection::Inverse,
        }
    }
}

/// How many firing modes a guard may compile to. Disjunctive normal form is
/// exponential in nested disjunctions, and a net nobody can read is worse
/// than a guard the author is told to compose differently.
pub const MAX_MODES: usize = 8;

/// How far a correlation may reach. Two steps covers parent, child and the
/// sibling composition; a longer chain turns enabledness into graph traversal
/// on every keystroke (D150.c OQ-4).
pub const MAX_HOP_CHAIN: usize = 2;

/// The path from the subject to the tokens a [`CorrelatedGroup`] may bind.
/// Steps compose left to right, so `parent` then `child` reaches a sibling.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Correlation {
    pub hops: Vec<Hop>,
    /// Whether the subject is removed from the tokens the hop reaches — what
    /// makes `sibling(p)` mean "some OTHER block", so a block never matches
    /// through itself.
    #[serde(default)]
    pub exclude_subject: bool,
}

/// The predicate an arc puts on its place's cell.
///
/// Typed rather than a raw [`Pattern`] so the compiler and the evaluator
/// cannot drift: every shape `holon_net::guards` turns into an arc is a shape
/// `holon_net::enabledness` can decide, checked by the compiler.
///
/// A refinement carries EVERYTHING its decision depends on. A place is only
/// `relation.field`, so a predicate that also names a key inside that field
/// must carry the key: without it `property("a") == x` and
/// `property("b") == x` compile to the same arc and the net answers one for
/// the other.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Refinement {
    /// The place's cell compares to a literal.
    Cell { op: CmpOp, rhs: Value },
    /// One KEY inside the place's map compares to a literal.
    Keyed { key: String, op: CmpOp, rhs: Value },
    /// The place is `block.tags` and it carries this tag.
    HasTag(String),
}

/// What a firing does to the tokens in an arc's place.
///
/// Only the vocabulary is carried here: nothing executes consumption — the
/// flows mirror what descriptors already declare (`MarkingDelta`,
/// `TransitionArcs`) so the analyses can read them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Flow {
    Read,
    Produce,
    Consume,
    /// Consume-and-produce on the same place family (a move's placement
    /// tokens). Counts as both a write and a read in the analyses.
    Relocate,
}

/// The three coarse token aspects of ADR 0032 §4. Field-granular tokens are
/// addressed by their [`ArcPlace`] directly and need no variant here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aspect {
    Structural,
    Text,
    Existence,
}

/// Which source declaration an arc was compiled from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArcOrigin {
    /// `#[reads]` on the operation.
    DeclaredRead,
    /// `#[emits]` on the operation.
    DeclaredEmit,
    /// The operation's `#[marking_delta]`, lowered through [`aspect_places`].
    Delta { aspect: Aspect },
    /// The guard subject's binding row.
    Subject,
    /// A guard conjunct the arc language expresses; the conjunct rides along
    /// as the arc's refinement.
    GuardRefinement,
    /// An arc inside a [`CorrelatedGroup`] — a cell of the entity the hop
    /// reaches, tested against the group's bound token.
    GuardHop,
    /// A place a guard names without its predicate being expressible as an
    /// arc; the predicate itself stays in [`NetTransition::residue`]. A guard
    /// reads every place it tests, so these arcs keep the read set honest.
    GuardFootprint,
    /// A rule's `emit:` output.
    RuleEmit,
}

/// One arc of the compiled net.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetArc {
    pub place: ArcPlace,
    pub flow: Flow,
    pub origin: ArcOrigin,
    /// A predicate narrowing which tokens in the place this arc matches.
    /// Opaque to the analyses: place identity stays `relation.field`, so
    /// ignoring the refinement widens what they report, never narrows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refinement: Option<Refinement>,
    /// Set when this arc belongs to a [`CorrelatedGroup`], naming the token
    /// the whole group is tested against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<BindingVar>,
}

/// Arcs that must all hold of ONE token reached by `correlation`.
///
/// The grouping is the point, not bookkeeping. Giving each correlated arc its
/// own correlation would evaluate `∃x.(a(x) ∧ b(x))` as
/// `(∃x.a(x)) ∧ (∃x.b(x))`, which is a strictly weaker predicate: two
/// different entities may satisfy the two halves. Binding the group to one
/// token makes that state unrepresentable rather than merely asserted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CorrelatedGroup {
    pub binding: BindingVar,
    pub correlation: Correlation,
    pub arcs: Vec<NetArc>,
}

/// One disjunct of a transition's guard in disjunctive normal form: a set of
/// arcs that together enable a firing. A transition whose guard carries no
/// disjunction has exactly one mode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransitionMode {
    /// Arcs tested against the subject's own row.
    pub arcs: Vec<NetArc>,
    /// Arcs tested against a second entity, one bound token per group.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hops: Vec<CorrelatedGroup>,
}

impl TransitionMode {
    pub fn new(arcs: Vec<NetArc>) -> Self {
        TransitionMode {
            arcs,
            hops: Vec::new(),
        }
    }

    /// Every arc of this mode, subject-local and correlated alike.
    pub fn all_arcs(&self) -> impl Iterator<Item = &NetArc> {
        self.arcs
            .iter()
            .chain(self.hops.iter().flat_map(|g| g.arcs.iter()))
    }
}

/// A guard predicate the arc language cannot express — a negation, an
/// existence test, a comparison against a builtin, or a guard that exceeded a
/// compiler bound. The places it names still appear as
/// [`ArcOrigin::GuardFootprint`] read arcs; only the predicate is opaque.
///
/// The cause is RECORDED where it is known rather than re-derived from the
/// predicate later: an over-budget guard's cause cannot be read off its shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuardResidue {
    pub predicate: Pattern,
    pub cause: crate::guards::ResidueCause,
}

/// Which declaration half an operation left undeclared.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UndeclaredHalf {
    Arcs,
    MarkingDelta,
}

/// Fail-closed analyzability. `Unanalyzable` means "cannot say", never
/// "touches nothing": the analyses must surface such a transition in every
/// report instead of silently treating it as conflict-free.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Analyzability {
    Analyzable,
    Unanalyzable { undeclared: Vec<UndeclaredHalf> },
}

/// One transition of the compiled net.
///
/// The arcs live in [`Self::modes`]; [`Self::arcs`] is their union, which is
/// what the analyses read. Storing only the modes is what makes
/// "`arcs` = union of the modes" true by construction rather than asserted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetTransition {
    pub source: TransitionSource,
    pub analyzability: Analyzability,
    pub modes: Vec<TransitionMode>,
    pub residue: Vec<GuardResidue>,
}

impl NetTransition {
    /// # Panics
    /// On an empty `modes`. A transition that can fire in no way at all is a
    /// compiler bug, not a net a caller should be able to build: every
    /// transition has at least the mode its declared arcs form.
    pub fn new(
        source: TransitionSource,
        analyzability: Analyzability,
        modes: Vec<TransitionMode>,
        residue: Vec<GuardResidue>,
    ) -> Self {
        assert!(
            !modes.is_empty(),
            "transition {} compiled to no firing mode at all; a guard in DNF has at least one \
             disjunct and an unguarded transition has exactly one mode",
            source.key()
        );
        NetTransition {
            source,
            analyzability,
            modes,
            residue,
        }
    }

    /// This transition's identity, derived from its source — the net stores
    /// no second copy that could drift.
    pub fn key(&self) -> TransitionKey {
        self.source.key()
    }

    /// Every arc of every mode, deduplicated, in mode order.
    ///
    /// The union is the sound over-approximation the analyses want: a place
    /// any mode touches is a place a firing may touch.
    pub fn arcs(&self) -> Vec<&NetArc> {
        let mut out: Vec<&NetArc> = Vec::new();
        for arc in self.modes.iter().flat_map(TransitionMode::all_arcs) {
            if !out.contains(&arc) {
                out.push(arc);
            }
        }
        out
    }

    /// The places this transition may write: every `Produce` or `Relocate`
    /// arc's place.
    pub fn written_places(&self) -> BTreeSet<&ArcPlace> {
        self.arcs()
            .into_iter()
            .filter(|a| matches!(a.flow, Flow::Produce | Flow::Relocate))
            .map(|a| &a.place)
            .collect()
    }

    /// The places this transition's enabledness depends on: every `Read`,
    /// `Consume`, or `Relocate` arc's place, guard footprints included.
    pub fn read_places(&self) -> BTreeSet<&ArcPlace> {
        self.arcs()
            .into_iter()
            .filter(|a| matches!(a.flow, Flow::Read | Flow::Consume | Flow::Relocate))
            .map(|a| &a.place)
            .collect()
    }
}

/// The whole compiled net — a pure function of the descriptor catalog and
/// the parsed rules, rebuilt on demand and never stored.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledNet {
    pub transitions: Vec<NetTransition>,
}

impl CompiledNet {
    /// The transition a report's key names. `None` only when the key belongs
    /// to a different net than the report it came from.
    pub fn transition(&self, key: &TransitionKey) -> Option<&NetTransition> {
        self.transitions.iter().find(|t| &t.key() == key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NetCompileError {
    #[error(
        "the guard of {transition} needs {modes} firing modes, over the limit of {limit}: \
         disjunctive normal form is exponential in nested disjunctions. Compose the guard from \
         named sub-patterns, or hoist the shared conjuncts out of the disjunction"
    )]
    TooManyModes {
        transition: String,
        modes: usize,
        limit: usize,
    },

    #[error(
        "the guard of {transition} hops {depth} times, over the limit of {limit}: a longer \
         chain turns enabledness into a graph traversal on every keystroke. Use `sibling(…)` \
         for the parent-then-child reach, or split the predicate"
    )]
    HopChainTooLong {
        transition: String,
        depth: usize,
        limit: usize,
    },

    #[error(
        "no place mapping for aspect {aspect:?} of kind {kind:?}: only `block` aspect tokens \
         have declared carrier places (extend `aspect_places` when a delta first declares \
         another kind)"
    )]
    AspectUnmapped { kind: ArcRelation, aspect: Aspect },

    #[error(
        "two sources compile to the transition {key}: an operation is identified by its \
         (entity, op) and a rule by its block, so a repeat means two providers claim one \
         identity — resolve the claim rather than letting one silently shadow the other"
    )]
    DuplicateTransition { key: TransitionKey },

    #[error(
        "operation {op:?} cannot be lowered to a transition: {source}. \
         `holon_core::classify_for_net` refuses this shape at the catalog boundary, so a \
         descriptor reaching compilation with it means the catalog was bypassed — give it a \
         dotless entity name rather than letting it sit outside every net analysis"
    )]
    UnkeyableOperation {
        op: String,
        #[source]
        source: crate::bridge::NetError,
    },
}

/// The concrete places that carry one coarse aspect's tokens (ADR 0032 §4),
/// for the `block` kind:
///
/// - structural → `block.parent_id`, `block.sort_key` (the placement columns)
/// - text → `block.content`
/// - existence → `block.id` (the row's identity)
pub fn aspect_places(kind: &ArcRelation, aspect: Aspect) -> Result<Vec<ArcPlace>, NetCompileError> {
    if kind.as_str() != block::RELATION {
        return Err(NetCompileError::AspectUnmapped {
            kind: kind.clone(),
            aspect,
        });
    }
    Ok(match aspect {
        Aspect::Structural => vec![
            ArcPlace::new(block::RELATION, block::PARENT_ID),
            ArcPlace::new(block::RELATION, block::SORT_KEY),
        ],
        Aspect::Text => vec![ArcPlace::new(block::RELATION, block::CONTENT)],
        Aspect::Existence => vec![ArcPlace::new(block::RELATION, block::ID)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_aspects_map_to_their_carrier_places() {
        let kind = ArcRelation::block();
        let places = |aspect| {
            aspect_places(&kind, aspect)
                .unwrap()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            places(Aspect::Structural),
            ["block.parent_id", "block.sort_key"]
        );
        assert_eq!(places(Aspect::Text), ["block.content"]);
        assert_eq!(places(Aspect::Existence), ["block.id"]);
    }

    #[test]
    fn a_non_block_aspect_is_a_loud_error() {
        let err = aspect_places(&ArcRelation::new("integration"), Aspect::Structural).unwrap_err();
        assert!(
            matches!(err, NetCompileError::AspectUnmapped { .. }),
            "{err}"
        );
    }

    #[test]
    fn a_compiled_net_round_trips_through_serde() {
        let net = CompiledNet {
            transitions: vec![NetTransition {
                source: TransitionSource::Operation {
                    entity: crate::bridge::NetEntity::parse("block").expect("dotless"),
                    op: "set_field".into(),
                },
                analyzability: Analyzability::Analyzable,
                modes: vec![TransitionMode::new(vec![NetArc {
                    place: ArcPlace::new("block", "content"),
                    flow: Flow::Produce,
                    origin: ArcOrigin::DeclaredEmit,
                    refinement: None,
                    binding: None,
                }])],
                residue: vec![],
            }],
        };
        let json = serde_json::to_string(&net).unwrap();
        let back: CompiledNet = serde_json::from_str(&json).unwrap();
        assert_eq!(net, back);
    }
}
