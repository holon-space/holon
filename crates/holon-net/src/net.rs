//! The compiled net's vocabulary. Place identity is `relation.field`
//! ([`ArcPlace`]), shared verbatim with the declaration surface so the two
//! cannot drift.

use std::collections::BTreeSet;

use holon_pattern::arcs::ArcPlace;
use holon_pattern::arcs::ArcRelation;
use holon_pattern::pattern::Pattern;
use holon_pattern::schema::block;
use serde::Deserialize;
use serde::Serialize;

use crate::bridge::TransitionKey;
use crate::bridge::TransitionSource;

/// Reserved for correlated multi-arc bindings — the CPN-orthodox join ADR
/// 0032 §2 anticipates, where one transition's arcs unify variables across
/// entities. Nothing constructs this yet; reserving the slot means the join
/// arrives as new values, not a new schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BindingVar(pub String);

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
    pub refinement: Option<Pattern>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<BindingVar>,
}

/// A guard predicate the arc language cannot express — a hop to another
/// entity (`parent(…)`), a negated existence test, a disjunction. The places
/// it names still appear as [`ArcOrigin::GuardFootprint`] read arcs; only the
/// predicate is opaque.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuardResidue {
    pub predicate: Pattern,
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NetTransition {
    pub source: TransitionSource,
    pub analyzability: Analyzability,
    pub arcs: Vec<NetArc>,
    pub residue: Vec<GuardResidue>,
}

impl NetTransition {
    /// This transition's identity, derived from its source — the net stores
    /// no second copy that could drift.
    pub fn key(&self) -> TransitionKey {
        self.source.key()
    }

    /// The places this transition may write: every `Produce` or `Relocate`
    /// arc's place.
    pub fn written_places(&self) -> BTreeSet<&ArcPlace> {
        self.arcs
            .iter()
            .filter(|a| matches!(a.flow, Flow::Produce | Flow::Relocate))
            .map(|a| &a.place)
            .collect()
    }

    /// The places this transition's enabledness depends on: every `Read`,
    /// `Consume`, or `Relocate` arc's place, guard footprints included.
    pub fn read_places(&self) -> BTreeSet<&ArcPlace> {
        self.arcs
            .iter()
            .filter(|a| matches!(a.flow, Flow::Read | Flow::Consume | Flow::Relocate))
            .map(|a| &a.place)
            .collect()
    }

    /// Append `arc`, skipping an exact duplicate.
    pub(crate) fn push_arc(&mut self, arc: NetArc) {
        if !self.arcs.contains(&arc) {
            self.arcs.push(arc);
        }
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
                    entity: "block".into(),
                    op: "set_field".into(),
                },
                analyzability: Analyzability::Analyzable,
                arcs: vec![NetArc {
                    place: ArcPlace::new("block", "content"),
                    flow: Flow::Produce,
                    origin: ArcOrigin::DeclaredEmit,
                    refinement: None,
                    binding: None,
                }],
                residue: vec![],
            }],
        };
        let json = serde_json::to_string(&net).unwrap();
        let back: CompiledNet = serde_json::from_str(&json).unwrap();
        assert_eq!(net, back);
    }
}
