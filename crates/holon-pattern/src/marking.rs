//! Declared marking deltas (ADR 0032 §4): what an operation does to each
//! aspect's tokens, per entity kind.
//!
//! One entity carries several colored tokens, one per aspect, and the aspects
//! are not interchangeable. That asymmetry is in the types rather than in a
//! validator: [`TextFlow`] and [`ExistenceFlow`] have no consuming variant, so
//! "consume the text token" — which would assert exclusion where the CRDT
//! guarantees convergence — cannot be written down.
//!
//! A declaration is an ASSERTION AN ORACLE CHECKS, not documentation.
//! [`MarkingDelta::Static`] claims the effect happens on every successful
//! firing; [`MarkingDelta::Envelope`] claims it happens for some bindings and
//! names the parameters that decide. `Static` is therefore the strong form and
//! the one to reach for; `Envelope` buys tolerance by naming its price.
//!
//! [`MarkingDelta::Undeclared`] is the fail-closed statement, the analogue of
//! [`crate::arcs::TransitionArcs::Undeclared`]: "cannot say", never "changes
//! nothing".

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;

use serde::Deserialize;
use serde::Serialize;

use crate::arcs::ArcRelation;

/// What a transition does to the placement token of one entity kind. The
/// tree's one-parent invariant is this token's linearity, which is why this is
/// the one aspect with consuming variants.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuralFlow {
    Untouched,
    Reads,
    /// Places an entity that held no placement token.
    Produces,
    /// Takes the placement token and does not return one.
    Consumes,
    /// Takes the placement token and returns one elsewhere.
    Relocates,
}

/// What a transition does to the text token of one entity kind. Text tokens
/// are CRDT-shared and never exclusively held, so there is no consuming
/// variant.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextFlow {
    Untouched,
    Reads,
    Produces,
}

/// What a transition does to the existence token of one entity kind.
/// Existence tokens are never consumed: a deletion PRODUCES the absent state.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExistenceFlow {
    Untouched,
    Reads,
    Produces,
}

/// One entity kind's three aspect flows. Every aspect is stated; there is no
/// "aspect omitted" state to distinguish from `Untouched`.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KindDelta {
    /// The entity kind whose tokens move — the same vocabulary
    /// [`crate::arcs::ArcPlace`] names its relation in.
    pub kind: ArcRelation,
    pub structural: StructuralFlow,
    pub text: TextFlow,
    pub existence: ExistenceFlow,
}

/// An operation's declared marking delta. Non-defaultable, following the
/// [`crate::arcs::TransitionArcs`] house pattern.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MarkingDelta {
    /// Fail-closed. A consumer asked what this op does to the marking REFUSES.
    Undeclared,
    /// Every successful firing moves the declared tokens.
    Static { kinds: Vec<KindDelta> },
    /// The declared flows are an upper bound; `varies_by` names the parameters
    /// that decide which of them a given binding actually takes. Every
    /// parameter named must be one the operation accepts — the macro checks
    /// that against the method signature.
    Envelope {
        kinds: Vec<KindDelta>,
        varies_by: Vec<String>,
    },
}

impl MarkingDelta {
    /// The declared per-kind flows, or `None` when the op is undeclared.
    /// `None` and `Some(&[])` mean different things: "cannot say" vs "moves no
    /// kind's tokens".
    pub fn kinds(&self) -> Option<&[KindDelta]> {
        match self {
            MarkingDelta::Undeclared => None,
            MarkingDelta::Static { kinds } | MarkingDelta::Envelope { kinds, .. } => Some(kinds),
        }
    }

    /// The declaration for one kind, if this op declares one for it.
    pub fn for_kind(&self, kind: &ArcRelation) -> Option<&KindDelta> {
        self.kinds()?.iter().find(|k| &k.kind == kind)
    }

    /// Whether an unobserved effect is admissible — true only for an envelope,
    /// whose whole content is "this binding may not move what the declaration
    /// permits".
    fn tolerates_inaction(&self) -> bool {
        matches!(self, MarkingDelta::Envelope { .. })
    }

    /// Compare one kind's declaration against what a firing was observed to do.
    /// The caller supplies the observation; this decides whether the
    /// declaration told the truth about it.
    pub fn check_observation(
        &self,
        kind: &ArcRelation,
        observed: &ObservedDelta,
    ) -> Result<(), DeltaViolation> {
        let Some(declared) = self.for_kind(kind) else {
            return Err(DeltaViolation::NoDeclarationForKind { kind: kind.clone() });
        };
        let lax = self.tolerates_inaction();

        if !declared.structural.permits(&observed.structural, lax) {
            return Err(DeltaViolation::Structural {
                kind: kind.clone(),
                declared: declared.structural,
                observed: observed.structural.clone(),
            });
        }
        if !declared.text.permits(observed.text, lax) {
            return Err(DeltaViolation::Text {
                kind: kind.clone(),
                declared: declared.text,
                observed: observed.text,
            });
        }
        if !declared.existence.permits(observed.existence, lax) {
            return Err(DeltaViolation::Existence {
                kind: kind.clone(),
                declared: declared.existence,
                observed: observed.existence,
            });
        }
        Ok(())
    }
}

/// One row-level placement change a firing was observed to make. The set of
/// these is empty exactly when placement did not move.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StructuralEvidence {
    /// A row that holds a placement now and held none before.
    Placed,
    /// A row that held a placement and holds none now.
    Unplaced,
    /// A surviving row whose placement changed.
    Moved,
}

/// Whether an aspect's observable state differs across a firing.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AspectChange {
    Unchanged,
    Changed,
}

/// What one firing was observed to do to one entity kind's rows.
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservedDelta {
    pub structural: BTreeSet<StructuralEvidence>,
    pub text: AspectChange,
    pub existence: AspectChange,
}

/// Where an entity sits among its siblings — the state the placement token
/// stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    pub parent: String,
    pub order: String,
}

/// One entity's three aspects as a store reports them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowState {
    /// `None` when the entity holds no placement token.
    pub placement: Option<Placement>,
    pub text: String,
}

/// Reduce two snapshots of one entity kind's rows to what the firing between
/// them did to each aspect.
///
/// A departing row's text and placement tokens leave with its existence token,
/// so only the `Unplaced` half of its structural evidence is read and its text
/// is not counted as having moved.
pub fn observe(
    before: &BTreeMap<String, RowState>,
    after: &BTreeMap<String, RowState>,
) -> ObservedDelta {
    let mut structural = BTreeSet::new();
    let mut text = AspectChange::Unchanged;

    for (id, row) in after {
        match before.get(id) {
            None => {
                if row.placement.is_some() {
                    structural.insert(StructuralEvidence::Placed);
                }
                text = AspectChange::Changed;
            }
            Some(was) => {
                if was.placement != row.placement {
                    structural.insert(StructuralEvidence::Moved);
                }
                if was.text != row.text {
                    text = AspectChange::Changed;
                }
            }
        }
    }
    for (id, row) in before {
        if !after.contains_key(id) && row.placement.is_some() {
            structural.insert(StructuralEvidence::Unplaced);
        }
    }

    let existence = if before.keys().eq(after.keys()) {
        AspectChange::Unchanged
    } else {
        AspectChange::Changed
    };

    ObservedDelta {
        structural,
        text,
        existence,
    }
}

impl StructuralFlow {
    /// `lax` admits an unobserved effect, which is what an envelope declares.
    pub fn permits(&self, observed: &BTreeSet<StructuralEvidence>, lax: bool) -> bool {
        if observed.is_empty() {
            return lax || matches!(self, StructuralFlow::Untouched | StructuralFlow::Reads);
        }
        match self {
            StructuralFlow::Untouched | StructuralFlow::Reads => false,
            StructuralFlow::Produces => observed.iter().all(|e| *e == StructuralEvidence::Placed),
            StructuralFlow::Consumes => observed.iter().all(|e| *e == StructuralEvidence::Unplaced),
            StructuralFlow::Relocates => true,
        }
    }
}

impl TextFlow {
    pub fn permits(&self, observed: AspectChange, lax: bool) -> bool {
        match (self, observed) {
            (TextFlow::Untouched | TextFlow::Reads, AspectChange::Unchanged) => true,
            (TextFlow::Untouched | TextFlow::Reads, AspectChange::Changed) => false,
            (TextFlow::Produces, AspectChange::Changed) => true,
            (TextFlow::Produces, AspectChange::Unchanged) => lax,
        }
    }
}

impl ExistenceFlow {
    pub fn permits(&self, observed: AspectChange, lax: bool) -> bool {
        match (self, observed) {
            (ExistenceFlow::Untouched | ExistenceFlow::Reads, AspectChange::Unchanged) => true,
            (ExistenceFlow::Untouched | ExistenceFlow::Reads, AspectChange::Changed) => false,
            (ExistenceFlow::Produces, AspectChange::Changed) => true,
            (ExistenceFlow::Produces, AspectChange::Unchanged) => lax,
        }
    }
}

/// How a declaration and an observation disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeltaViolation {
    NoDeclarationForKind {
        kind: ArcRelation,
    },
    Structural {
        kind: ArcRelation,
        declared: StructuralFlow,
        observed: BTreeSet<StructuralEvidence>,
    },
    Text {
        kind: ArcRelation,
        declared: TextFlow,
        observed: AspectChange,
    },
    Existence {
        kind: ArcRelation,
        declared: ExistenceFlow,
        observed: AspectChange,
    },
}

impl fmt::Display for DeltaViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeltaViolation::NoDeclarationForKind { kind } => write!(
                f,
                "the op moved {kind} tokens but its marking delta declares nothing for that kind"
            ),
            DeltaViolation::Structural {
                kind,
                declared,
                observed,
            } => write!(
                f,
                "{kind} placement: declared {declared:?}, observed {observed:?}"
            ),
            DeltaViolation::Text {
                kind,
                declared,
                observed,
            } => write!(
                f,
                "{kind} text: declared {declared:?}, observed {observed:?}"
            ),
            DeltaViolation::Existence {
                kind,
                declared,
                observed,
            } => write!(
                f,
                "{kind} existence: declared {declared:?}, observed {observed:?}"
            ),
        }
    }
}

impl std::error::Error for DeltaViolation {}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(
        structural: &[StructuralEvidence],
        text: AspectChange,
        existence: AspectChange,
    ) -> ObservedDelta {
        ObservedDelta {
            structural: structural.iter().copied().collect(),
            text,
            existence,
        }
    }

    fn declare(
        structural: StructuralFlow,
        text: TextFlow,
        existence: ExistenceFlow,
    ) -> Vec<KindDelta> {
        vec![KindDelta {
            kind: ArcRelation::block(),
            structural,
            text,
            existence,
        }]
    }

    #[test]
    fn a_static_declaration_requires_the_effect_to_actually_happen() {
        let delta = MarkingDelta::Static {
            kinds: declare(
                StructuralFlow::Produces,
                TextFlow::Untouched,
                ExistenceFlow::Produces,
            ),
        };
        let inert = observed(&[], AspectChange::Unchanged, AspectChange::Unchanged);
        assert!(matches!(
            delta.check_observation(&ArcRelation::block(), &inert),
            Err(DeltaViolation::Structural { .. })
        ));
    }

    /// The tolerance an envelope buys, and the reason it must name its
    /// parameters: the same inert firing that refutes a static declaration is
    /// what an envelope exists to admit.
    #[test]
    fn an_envelope_admits_the_firing_that_moves_nothing() {
        let delta = MarkingDelta::Envelope {
            kinds: declare(
                StructuralFlow::Produces,
                TextFlow::Produces,
                ExistenceFlow::Produces,
            ),
            varies_by: vec!["field".to_string()],
        };
        let inert = observed(&[], AspectChange::Unchanged, AspectChange::Unchanged);
        assert_eq!(
            delta.check_observation(&ArcRelation::block(), &inert),
            Ok(())
        );
    }

    /// An envelope is tolerance about WHETHER, never about WHICH: a declared
    /// producer that consumed a placement is refuted under either variant.
    #[test]
    fn an_envelope_does_not_admit_the_opposite_effect() {
        let delta = MarkingDelta::Envelope {
            kinds: declare(
                StructuralFlow::Produces,
                TextFlow::Untouched,
                ExistenceFlow::Produces,
            ),
            varies_by: vec!["field".to_string()],
        };
        let consumed = observed(
            &[StructuralEvidence::Unplaced],
            AspectChange::Unchanged,
            AspectChange::Changed,
        );
        assert!(matches!(
            delta.check_observation(&ArcRelation::block(), &consumed),
            Err(DeltaViolation::Structural { .. })
        ));
    }

    #[test]
    fn relocates_accepts_a_placement_that_moved() {
        let delta = MarkingDelta::Static {
            kinds: declare(
                StructuralFlow::Relocates,
                TextFlow::Untouched,
                ExistenceFlow::Reads,
            ),
        };
        let moved = observed(
            &[StructuralEvidence::Moved],
            AspectChange::Unchanged,
            AspectChange::Unchanged,
        );
        assert_eq!(
            delta.check_observation(&ArcRelation::block(), &moved),
            Ok(())
        );
    }

    /// `Reads` is a claim of no effect, so a read-only aspect that changed is
    /// as much a contradiction as a wrong-direction structural flow.
    #[test]
    fn a_read_only_aspect_that_changed_is_refuted() {
        let delta = MarkingDelta::Static {
            kinds: declare(
                StructuralFlow::Relocates,
                TextFlow::Reads,
                ExistenceFlow::Reads,
            ),
        };
        let text_moved = observed(
            &[StructuralEvidence::Moved],
            AspectChange::Changed,
            AspectChange::Unchanged,
        );
        assert!(matches!(
            delta.check_observation(&ArcRelation::block(), &text_moved),
            Err(DeltaViolation::Text { .. })
        ));
    }

    /// `Undeclared` and a declaration with no kinds are distinguishable, which
    /// is what makes silence a stated fact rather than an empty claim.
    #[test]
    fn undeclared_and_declared_empty_are_distinguishable() {
        assert_eq!(MarkingDelta::Undeclared.kinds(), None);
        assert_eq!(
            MarkingDelta::Static { kinds: vec![] }.kinds(),
            Some([].as_slice())
        );
    }

    #[test]
    fn a_kind_the_declaration_never_mentions_is_an_error_not_a_pass() {
        let delta = MarkingDelta::Static {
            kinds: declare(
                StructuralFlow::Relocates,
                TextFlow::Untouched,
                ExistenceFlow::Reads,
            ),
        };
        let err = delta
            .check_observation(
                &ArcRelation::clock(),
                &observed(&[], AspectChange::Unchanged, AspectChange::Unchanged),
            )
            .expect_err("clock is not declared");
        assert!(matches!(err, DeltaViolation::NoDeclarationForKind { .. }));
    }

    #[test]
    fn a_declaration_round_trips_through_serde() {
        let delta = MarkingDelta::Envelope {
            kinds: declare(
                StructuralFlow::Relocates,
                TextFlow::Produces,
                ExistenceFlow::Produces,
            ),
            varies_by: vec!["field".to_string()],
        };
        let json = serde_json::to_string(&delta).expect("serialize");
        let back: MarkingDelta = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, delta);
    }
}
