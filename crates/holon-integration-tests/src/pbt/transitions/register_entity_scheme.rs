//! Transition: an MCP sidecar provider registers an entity type at runtime.
//!
//! @pbt rung dispatch
//!   calls the agent-facing `create_entity_type` MCP tool over a real rmcp
//!   transport against the container's own `TypeRegistry` — the rung an
//!   integration connects on; there is no user gesture for it.
//! @pbt covers entity-scheme-registration — a registration can land ANYWHERE in
//!   the sequence, before or after the `[[t-widget:…]]` links it would claim.
//!
//! Bug #98 was classified COVERAGE precisely because no transition could do
//! this: the keystone could never generate a registration-vs-ingest
//! interleaving, so a link ingested before its scheme existed was unreachable
//! by construction.
//!
//! The reference effect is DELIBERATELY inert — it records the name and
//! nothing else. `block_links` is derived registry-independently
//! (`holon_api::derive_block_links`), so the link oracle must predict the same
//! rows before and after a registration. An oracle that changed here would be
//! asserting the bug rather than the fix.

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefEntitySchemes;
use holon_pbt_core::capabilities::RefEntitySchemesMut;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// The entity types this transition may mint, in SQL table spelling.
///
/// MULTI-WORD on purpose: `TypeRegistry` is keyed by table name (underscored)
/// while a URI scheme is hyphenated, and bug #71 was exactly that join
/// breaking. A single-word name spells both sides identically and so cannot
/// tell a working join from a broken one.
pub const ENTITY_NAMES: [&str; 2] = ["t_widget", "cc_session"];

/// The URI-scheme spelling of `ENTITY_NAMES[0]` — the scheme
/// `generators::typing_text_strategy` types links against, so a generated
/// `[[t-widget:…]]` and a generated registration name the SAME entity.
pub const LINKED_SCHEME: &str = "t-widget";

/// Register one entity type through the `create_entity_type` MCP tool.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RegisterEntityScheme {
    /// SQL table spelling, drawn from [`ENTITY_NAMES`].
    pub entity_name: String,
}

impl<R: RefLifecycle + RefEntitySchemes> TransitionFactory<R> for RegisterEntityScheme {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let unregistered: Vec<String> = ENTITY_NAMES
            .iter()
            .filter(|name| !state.entity_scheme_registered(name))
            .map(|name| (*name).to_string())
            .collect();
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!unregistered.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| {
            let strat = proptest::sample::select(unregistered)
                .prop_map(|entity_name| RegisterEntityScheme { entity_name })
                .boxed();
            // Rare event: installing an integration happens once in a
            // vault's life, and a heavy weight would crowd out the editing
            // that produces the links whose registration-independence is
            // the property under test.
            (1, strat)
        })
    }
}

impl<R: RefLifecycle + RefEntitySchemes + RefEntitySchemesMut> TransitionRef<R>
    for RegisterEntityScheme
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        // Re-checked here, not only at generation: the shrinker reorders and
        // drops transitions, so a draw that was the FIRST registration of this
        // entity can be revalidated against a state that already registered it
        // — and `TypeRegistry::register` on a live name is not a no-op.
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                !state.entity_scheme_registered(&self.entity_name),
                Reason::PreconditionFailed,
            ),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.note_entity_scheme_registered(&self.entity_name);
    }
}

crate::cap_transition! {
    RegisterEntityScheme: holon_pbt_core::capabilities::SutEntityTypeRegister,
    where R: [ RefLifecycle + RefEntitySchemes + RefEntitySchemesMut ],
    |me, _state, sut| {
        sut.register_entity_type(&me.entity_name).await;
    }
    sql_budget: |_me, _state| {
        // One extension-table CREATE plus the schema-availability bookkeeping
        // the tool performs after registering the type.
        ExpectedSql {
            reads: 4,
            writes: 0,
            ddl: 1,
            tolerance: 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use holon_pbt_core::capabilities::SutEntityTypeRegister;

    use super::*;
    use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;

    /// Reachability floor: the variant must actually be DRAWN from the
    /// keystone's own alphabet on the full-headless wiring. Cap-drawability
    /// (`non_vacuity_guard`) is necessary but not sufficient — a precondition
    /// that never holds would still make the transition silently dead.
    #[test]
    fn variant_is_drawn_from_the_composed_alphabet() {
        use proptest::strategy::ValueTree;
        use proptest::test_runner::TestRunner;
        use proptest_state_machine::ReferenceStateMachine;

        let state = crate::pbt::composed::wide_e2e::wide_e2e_ref();
        let strat = crate::pbt::composed::wide_e2e::WideE2EMachine::transitions(&state);
        let mut runner = TestRunner::deterministic();
        let hits = (0..2000)
            .filter(|_| {
                matches!(
                    strat
                        .new_tree(&mut runner)
                        .expect("composed alphabet draws")
                        .current(),
                    crate::pbt::transitions::E2ETransition::RegisterEntityScheme(_)
                )
            })
            .count();
        assert!(
            hits > 0,
            "RegisterEntityScheme was never drawn in 2000 samples of the full-headless \
             alphabet — the registration-vs-ingest interleaving bug #98 needs is still \
             ungeneratable"
        );
    }

    /// The SUT rung end to end: the `create_entity_type` MCP tool call must
    /// land in the container's OWN `TypeRegistry` — the one the link classifier
    /// reads — for every name the generator can draw. A registration visible
    /// only to a registry the classifier never consults would make the whole
    /// transition theatre.
    #[tokio::test(flavor = "multi_thread")]
    async fn mcp_tool_registration_reaches_the_link_classifier() {
        use holon_api::link_parser::LinkTarget;

        let comp = HeadlessFrontendComponent::new(
            &[("doc0.org", "#+ID: ref-doc-0\n* Doc zero\n")],
            Duration::from_millis(300),
        )
        .await;
        let registry = comp.type_registry().await;
        assert!(
            matches!(
                registry.link_target_classifier().classify("t-widget:abc"),
                LinkTarget::UnknownScheme(_)
            ),
            "precondition: the scheme must be unclaimed before the tool runs"
        );

        for name in ENTITY_NAMES {
            comp.register_entity_type(name).await;
            assert!(
                registry.contains(name),
                "create_entity_type('{name}') must register in the container's live registry"
            );
        }

        assert!(
            matches!(
                registry.link_target_classifier().classify("t-widget:abc"),
                LinkTarget::Resolved(_)
            ),
            "the hyphenated scheme must fold onto the underscored table name — the #71 join"
        );
    }
}
