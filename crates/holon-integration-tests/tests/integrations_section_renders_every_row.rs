//! The seeded Integrations section renders ONE row per matching table row.
//!
//! Entry `2026-08-18-integrations-section-renders-one-of-four-rows`: the live
//! app painted a SINGLE row while `integration_state` held four matching ones.
//!
//! # The tier this rung drives, and why it is not the obvious one
//!
//! `ReactiveEngine::snapshot_resolved` does not expand a `live_query`: its
//! builder stores a static slot, the item_template interpreted against
//! `with_data_rows(vec![])` (`shared_live_query_build`), and the platform layer
//! mounts the real watcher from the node props. A rung reading the resolved
//! block tree therefore sees one template instance whether the section is
//! healthy or not.
//!
//! `watch_query_live` (`holon-frontend/src/reactive.rs`) is the tier that
//! interprets the section's render expression against the delivered rows, and
//! is what `ReactiveShell` renders. This rung drives that, with the section's
//! own sql and item_template, so it moves with the seeded layout rather than
//! with a copy of it.
//!
//! `live_query` applies its item_template as the WHOLE render expression, once,
//! against the whole delivered row set — so only a COLLECTION widget iterates
//! the rows, and a scalar template renders a single instance.
//!
//! @pbt kind harness
//! @pbt covers integrations-section-renders-every-row — the rendered tree
//! carries one `integration:` row per matching `integration_state` row
//! @pbt slips-if-removed the section silently paints a subset of the
//! integrations the user has, and every layer's own rung stays green because
//! each layer did its job

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::di::DbHandleProvider;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive::ReactiveEngine;
use holon_frontend::view_model::ViewModel;
use holon_integration_tests::TestEnvironment;

/// The dogfooded shape was four rows in the table and one on screen, so a rung
/// that ran against fewer than this many rows could not have caught it.
const MIN_ROWS_FOR_TEETH: usize = 4;

/// Every `integration:` row id the rendered tree carries, deduplicated.
///
/// Deduplicated because one row renders several nodes that all carry its id
/// (the row container, its texts, its switch); what is being counted is ROWS,
/// not nodes.
fn rendered_integration_rows(vm: &ViewModel) -> BTreeSet<String> {
    fn walk(vm: &ViewModel, out: &mut BTreeSet<String>) {
        if let Some(id) = vm.row_id() {
            if id.starts_with("integration:") {
                out.insert(id);
            }
        }
        for child in vm.children() {
            walk(child, out);
        }
    }
    let mut out = BTreeSet::new();
    walk(vm, &mut out);
    out
}

#[test]
fn the_seeded_section_renders_one_row_per_matching_table_row() {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(run(runtime.clone()));
}

async fn run(runtime: Arc<tokio::runtime::Runtime>) {
    let env = TestEnvironment::new(runtime).expect("new TestEnvironment");
    env.start_app(false).await.expect("start_app");

    let db = env
        .injector()
        .expect("start_app must capture the injector")
        .resolve::<dyn DbHandleProvider>()
        .handle();
    db.transition_to_ready()
        .await
        .expect("transition the actor to Ready");

    // The boot projector has already mirrored every bundled provider, which is
    // the state the dogfooding session was in. Switch them all ON so the rows
    // are alike and the only thing that can differ is how many got rendered.
    db.execute_values(
        "UPDATE integration_state SET enabled = 1, enabled_state = 'on'",
        vec![],
    )
    .await
    .expect("switch every mirrored integration on");

    // The ORACLE is the table itself, not a hardcoded list: the property is
    // "one rendered row per matching table row", and reading the table keeps it
    // true when the bundle changes.
    let want: BTreeSet<String> = db
        .query("SELECT id FROM integration_state", Default::default())
        .await
        .expect("read the rows the section queries")
        .iter()
        .map(|r| {
            r.get("id")
                .and_then(|v| v.as_string())
                .expect("integration_state.id")
                .to_string()
        })
        .collect();
    assert!(
        want.len() >= MIN_ROWS_FOR_TEETH,
        "this rung needs at least {MIN_ROWS_FOR_TEETH} mirrored integrations to have teeth; the \
         table holds {}. A one-row table would pass while rendering one row, which is the very \
         defect being guarded.",
        want.len()
    );

    let reactive: Arc<ReactiveEngine> = env
        .reactive_engine
        .get()
        .expect("start_app must resolve a ReactiveEngine")
        .clone();
    let services: Arc<dyn BuilderServices> = reactive.clone();

    // The section's OWN item_template, parsed from the shared source the seed
    // embeds. Restating it here would let the seed drift into the defect while
    // this rung stayed green.
    let item_template = holon_api::render_dsl::parse_render_dsl(
        holon_app::integrations_section::SIDEBAR_ITEM_TEMPLATE,
    )
    .expect("the section's item_template must parse");

    let (key, live) = reactive.watch_query_live(
        holon_app::integrations_section::SIDEBAR_SQL.to_string(),
        holon_api::QueryLanguage::HolonSql,
        item_template,
        None,
        services,
    );

    // The WATCH leg first, so a delivery failure cannot be mistaken for the
    // render failure this rung guards.
    let rows = reactive.ensure_watching(&key);
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut delivered = 0;
    while Instant::now() < deadline {
        delivered = rows.snapshot().1.len();
        if delivered >= want.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        delivered,
        want.len(),
        "precondition: the section's sql must DELIVER every row. It delivered {delivered} of {}. \
         That is a delivery defect, not the render defect this rung guards.",
        want.len()
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    let got = loop {
        let got = rendered_integration_rows(&live.tree.snapshot());
        if got == want {
            break got;
        }
        assert!(
            Instant::now() < deadline,
            "the Integrations section RENDERED {} of {} rows, while its watch DELIVERED all \
             {delivered}.\n  rendered: {:?}\n  in the table: {:?}\n  missing: {:?}\nThe rows are \
             delivered, so the loss is in how the section's item_template is applied to them. A \
             SCALAR item_template is interpreted ONCE against the whole row set; only a \
             COLLECTION widget iterates it. Bugfunnel \
             2026-08-18-integrations-section-renders-one-of-four-rows.",
            got.len(),
            want.len(),
            got,
            want,
            want.difference(&got).collect::<Vec<_>>(),
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    // Vacuity guard: an assertion over an empty rendered set would pass on a
    // section that never drew at all.
    assert_eq!(
        got.len(),
        want.len(),
        "the rung must observe every mirrored integration — a short set makes it vacuous"
    );

    drop(live);
}
