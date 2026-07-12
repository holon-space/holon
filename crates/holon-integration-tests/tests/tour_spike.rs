//! Onboarding-tour vertical-slice spike (see
//! `docs/Proposals/OnboardingTours-2026-07-12.md`).
//!
//! Proves, against the **real engine** and real block substrate, the seams a
//! production tour stands on — WITHOUT painting GPUI pixels (the overlay host
//! is designed, not built) and WITHOUT the wait-for subscription (the predicate
//! is evaluated directly). The geometry seam (anchor → rect via
//! `GeometryProvider`) is covered by the unit tests in `holon_frontend::tour`,
//! which run against the same trait the GPUI `BoundsRegistry` implements.
//!
//! Seams exercised here:
//!   1. Tour-as-data parses from a seeded org subtree into a typed `Tour`.
//!   2. Manual advance is an ordinary `set_field` op that persists (observed
//!      via the block snapshot the reads see).
//!   3. Action-gated advance ("create a block under X") is an engine-observable
//!      state change the `ChildCreatedUnder` predicate evaluates correctly.

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::tour::AdvanceCondition;
use holon_frontend::tour::AnchorSelector;
use holon_frontend::tour::StatePredicate;
use holon_frontend::tour::TourViewModel;
use holon_frontend::tour::WellKnownPanel;
use holon_frontend::tour::parse_tour;
use holon_integration_tests::TestEnvironmentBuilder;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

const TOUR_ORG: &str = concat!(
    "* Target Page\n",
    ":PROPERTIES:\n",
    ":ID: target-page\n",
    ":END:\n",
    "* Welcome Tour\n",
    ":PROPERTIES:\n",
    ":ID: tour-welcome\n",
    ":TAGS: Tour\n",
    ":END:\n",
    "** This is your sidebar.\n",
    ":PROPERTIES:\n",
    ":ID: tour-step-1\n",
    ":TOUR_ANCHOR: panel:sidebar\n",
    ":TOUR_ADVANCE: next\n",
    ":END:\n",
    "** This is the main panel.\n",
    ":PROPERTIES:\n",
    ":ID: tour-step-2\n",
    ":TOUR_ANCHOR: panel:main\n",
    ":TOUR_ADVANCE: next\n",
    ":END:\n",
    "** Create your first block under the target.\n",
    ":PROPERTIES:\n",
    ":ID: tour-step-3\n",
    ":TOUR_ANCHOR: block:target-page\n",
    ":TOUR_ADVANCE: observed:child-created-under(block:target-page)\n",
    ":END:\n",
);

/// Count non-page blocks whose parent id equals `parent` (bare id), read from
/// the block snapshot the reads observe — the same seam `non_page_block_rows`
/// uses.
async fn children_of(env: &holon_integration_tests::TestEnvironment, parent: &str) -> usize {
    let snap = env
        .session()
        .block_query()
        .snapshot()
        .await
        .expect("block snapshot");
    snap.iter_blocks()
        .filter(|b| b.parent_id.id() == parent)
        .count()
}

#[test]
fn tour_spike_parses_advances_and_gates_on_real_engine() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = TestEnvironmentBuilder::new()
            .with_org_file("tour.org", TOUR_ORG)
            .build(rt.clone())
            .await
            .expect("build env"); // the builder's build() already starts the app

        // ---- Seam 1: tour-as-data parses into a typed Tour ------------------
        let blocks = env
            .parse_org_file_blocks(None)
            .await
            .expect("parse org blocks");
        let root = blocks
            .iter()
            .find(|b| b.id.id() == "tour-welcome")
            .expect("tour root block present");
        let steps: Vec<_> = blocks
            .iter()
            .filter(|b| b.parent_id.id() == "tour-welcome")
            .cloned()
            .collect();
        assert_eq!(steps.len(), 3, "three step blocks under the tour root");

        let tour = parse_tour(root, &steps).expect("parse_tour");
        assert_eq!(tour.steps.len(), 3);
        assert_eq!(
            tour.steps[0].anchor,
            AnchorSelector::Panel(WellKnownPanel::Sidebar)
        );
        assert_eq!(tour.steps[1].advance, AdvanceCondition::Next);
        assert_eq!(
            tour.steps[2].advance,
            AdvanceCondition::Observed(StatePredicate::ChildCreatedUnder {
                under: AnchorSelector::Block(EntityUri::from_raw("target-page")),
            }),
            "gated step parses to a ChildCreatedUnder predicate"
        );

        // ---- Seam 2: manual advance is a set_field op that persists ---------
        // Production advances the persisted progress cursor via an op with
        // OpOrigin::User; here we drive the same OperationEngine set_field path
        // and confirm it lands in the substrate the reads observe.
        let mut params: HashMap<String, Value> = HashMap::new();
        params.insert("id".into(), Value::String("block:tour-welcome".into()));
        params.insert("field".into(), Value::String("tour_active_step".into()));
        params.insert("value".into(), Value::String("1".into()));
        env.execute_operation("block", "set_field", params)
            .await
            .expect("set_field advance op");

        let snap = env
            .session()
            .block_query()
            .snapshot()
            .await
            .expect("snapshot after advance");
        let persisted = snap
            .iter_blocks()
            .find(|b| b.id.id() == "tour-welcome")
            .and_then(|b| b.properties.get("tour_active_step").cloned());
        assert_eq!(
            persisted,
            Some(Value::String("1".into())),
            "advance op must persist the progress cursor"
        );

        // Mirror it in the pure projection.
        let mut vm = TourViewModel::new(tour.clone());
        assert_eq!(vm.active_index(), 0);
        vm.advance();
        assert_eq!(vm.active_index(), 1);

        // ---- Seam 3: action-gated advance observes a real op ----------------
        let baseline = children_of(&env, "target-page").await;
        assert_eq!(baseline, 0, "target page starts childless");

        // Drive the gated step to active and arm its baseline (production reads
        // this from the wait-for subscription's initial state).
        vm.advance(); // now on the gated step (index 2)
        assert_eq!(vm.active_index(), 2);
        vm.arm_observation(baseline);
        assert!(
            !vm.observed_gate_open(baseline),
            "gate closed before the user acts"
        );

        // The user performs the task: create a block under the target.
        env.create_block("target-child", "target-page", "my first block")
            .await
            .expect("create block under target");

        let after = children_of(&env, "target-page").await;
        assert_eq!(after, 1, "the create op is observable in engine state");
        assert!(
            vm.observed_gate_open(after),
            "gate opens once the observed child exists"
        );

        // Advancing past the last step finishes the tour.
        vm.advance();
        assert!(vm.is_finished());
    });
}
