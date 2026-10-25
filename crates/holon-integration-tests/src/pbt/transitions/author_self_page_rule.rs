//! Transition: an external editor adds a `holon_rule` block whose emit places
//! its output under the page that holds the rule.
//!
//! @pbt rung external
//!   the same org-file seam `BulkExternalAdd` drives; the live rule watcher
//!   discovers the rule and fires it once for the clock's day.
//! @pbt covers self-page-reference — a rule names the page that holds it,
//!   resolved to the nearest page ancestor when it fires

use std::collections::BTreeMap;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefClock;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefLayoutInteract;
use holon_pbt_core::capabilities::RefLayoutMutate;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutSeamMutate;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::cdc_tolerance;

/// `blocks` holds exactly one `holon_rule` block whose `emit.place` is the
/// self-page reference; `emits` is its `emit.name` template.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template(
    "an external add of the self-page rule {blocks} to document {doc_uri}, emitting {emits}"
)]
pub struct AuthorSelfPageRule {
    pub doc_uri: EntityUri,
    #[serde(with = "holon_api::block::block_wire_vec")]
    pub blocks: Vec<Block>,
    pub emits: String,
}

impl AuthorSelfPageRule {
    fn rule_block(&self) -> &Block {
        let rules: Vec<&Block> = self
            .blocks
            .iter()
            .filter(|b| b.source_language == Some(holon_api::SourceLanguage::HolonRule))
            .collect();
        assert_eq!(
            rules.len(),
            1,
            "AuthorSelfPageRule carries exactly one holon_rule block"
        );
        rules[0]
    }
}

fn nearest_page<R: RefBlockTree>(state: &R, start: &EntityUri) -> EntityUri {
    let mut at = start.clone();
    while !state.is_page_block(&at) {
        at = state
            .parent_of(&at)
            .unwrap_or_else(|| panic!("{start} has no page ancestor in the reference"));
    }
    at
}

impl<R: RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate + RefBlockTree + RefClock>
    TransitionFactory<R> for AuthorSelfPageRule
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(_: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Validated::fail(Reason::HandAuthoredOnly)
    }
}

impl<R: RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate + RefBlockTree + RefClock>
    TransitionRef<R> for AuthorSelfPageRule
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.has_document_uri(&self.doc_uri),
                Reason::NoDocumentsAvailable,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.bulk_add_blocks(&self.doc_uri, &self.blocks);
        let page = nearest_page(state, &self.rule_block().id);
        let content = self.emits.replace("{today}", &state.today());
        state.create_block_under_with_attributes(
            &page,
            &content,
            &holon_api::Tags::default(),
            &BTreeMap::new(),
        );
    }
}

crate::cap_transition! {
    AuthorSelfPageRule: SutSeamMutate,
    where R: [ RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate + RefBlockTree + RefClock ],
    |me, _state, sut| {
        sut.bulk_external_add(&me.doc_uri, &me.blocks).await;
    }
    sql_budget: |me, state| {
        let n = me.blocks.len() + 1;
        ExpectedSql {
            reads: holon_pbt_core::budget::cdc_drain_floor(state.document_count()),
            writes: n + 2,
            ddl: 0,
            tolerance: cdc_tolerance(state.block_count() + n, state.document_count()) + n * 3,
        }
    }
}
