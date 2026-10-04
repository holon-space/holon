//! Transition: press several Tab / Shift-Tab chords with no settle between
//! them, as a fast typist does.
//!
//! @pbt rung input-pipeline
//!   each key rides the same `SutBlockTreeWrite` chord path as `Indent` /
//!   `Outdent`; the harness settles once, after the last key, so a key is
//!   admitted while the projection of the previous one is still in flight.
//! @pbt covers structural-burst-decides-on-the-write-authority — a structural
//! op decides on the block and parent that earlier admitted keys produced, not
//! on a projection that has not caught up with them
//!
//! Hand-authored only: the reference model applies each key in order, exactly
//! as for the single-key transitions.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefEditorMirrorMut;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefFocusMut;
use holon_pbt_core::capabilities::RefGlobalFocus;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use holon_pbt_core::validation::Reason;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BurstKey {
    Indent(EntityUri),
    Outdent(EntityUri),
}

/// The keys in press order. A newtype so it can travel through a replay step
/// as compact JSON.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BurstKeys(pub Vec<BurstKey>);

holon_pbt_core::step_field_via_json!(
    BurstKeys,
    vec![
        BurstKeys(vec![BurstKey::Indent(EntityUri::block("x"))]),
        BurstKeys(vec![
            BurstKey::Indent(EntityUri::block("x")),
            BurstKey::Outdent(EntityUri::block("x")),
        ]),
    ]
);

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I press the structural keys {keys} without pausing")]
pub struct StructuralKeyBurst {
    pub keys: BurstKeys,
}

impl<R> TransitionFactory<R> for StructuralKeyBurst {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(_: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Validated::fail(Reason::HandAuthoredOnly)
    }
}

fn apply_key<
    R: RefBlockTree + RefBlockTreeMut + RefFocus + RefGlobalFocus + RefFocusMut + RefEditorMirrorMut,
>(
    key: &BurstKey,
    state: &mut R,
) {
    match key {
        BurstKey::Indent(id) => super::indent::indent_apply_to_ref(id, state),
        BurstKey::Outdent(id) => super::outdent::outdent_apply_to_ref(id, state),
    }
}

impl<
    R: RefBlockTree
        + RefBlockTreeMut
        + RefFocus
        + RefGlobalFocus
        + RefFocusMut
        + RefEditorMirrorMut
        + RefLifecycle
        + Clone,
> TransitionRef<R> for StructuralKeyBurst
{
    type Reason = Reason;

    /// Every key must be valid in the state the keys before it leave behind.
    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let mut after = state.clone();
        for key in &self.keys.0 {
            let valid = match key {
                BurstKey::Indent(id) => super::indent::indent_preconditions(id, &after),
                BurstKey::Outdent(id) => super::outdent::outdent_preconditions(id, &after),
            };
            if !valid.is_good() {
                return valid;
            }
            apply_key(key, &mut after);
        }
        Validated::Good(())
    }

    fn apply_to_ref(&self, state: &mut R) {
        for key in &self.keys.0 {
            apply_key(key, state);
        }
    }
}

crate::cap_transition! {
    StructuralKeyBurst: SutBlockTreeWrite,
    where R: [ RefBlockTree + RefLifecycle ],
    |me, _state, sut| {
        for key in &me.keys.0 {
            match key {
                BurstKey::Indent(id) => sut.apply_indent(id).await,
                BurstKey::Outdent(id) => sut.apply_outdent(id).await,
            }
        }
    }
    sql_budget: |me, state| {
        let one = expected_sql_for_kind(
            MutationKind::Update,
            state.active_watch_count(),
            state.block_count(),
            state.document_count(),
        );
        let keys = me.keys.0.len();
        holon_pbt_core::budget::ExpectedSql {
            reads: one.reads * keys,
            writes: one.writes * keys,
            ddl: one.ddl * keys,
            tolerance: (one.tolerance + 5) * keys,
        }
    }
}
