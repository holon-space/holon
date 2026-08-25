//! A Now-shaped flat live-query task list renders STRUCTURED task rows, not
//! bare title blobs.
//!
//! Entry `2026-08-25-flat-query-task-rows-render-as-page-title-blobs`: the
//! vault's Now list (a `holon_sql` SELECT over task blocks) painted every
//! result as one raw text widget — no TODO state toggle — because the
//! collection `tree_view`'s level-0 rule stamps `role: "page_title"` on every
//! parentless result row, and a flat cross-vault query makes EVERY row
//! parentless.
//!
//! The rung boots the production headless frontend over a two-file vault
//! (a tasks page plus a Now-shaped query page), focuses the query page the
//! way a user does (sidebar click), and judges the rendered widget tree with
//! the SAME core check the composed catalog's
//! `inv-viewmodel-task-rows-have-state-toggle` uses — so the dedicated rung
//! and the keystone judge the identical property.
//!
//! This lives as a dedicated rung (not a composed keystone sequence) because
//! focusing a query page whose results are cross-document rows false-reds
//! `inv-main-panel-rows-match-focus` today: the reference model has no
//! concept of query-page results, so panel rows that are ref-known
//! non-descendants of the focus root are indistinguishable from stale rows.
//! Modeling query-page results in the oracle is that invariant's own open
//! work; this rung reuses the keystone's component, snapshot IR, and check.
//!
//! @pbt kind harness
//! @pbt covers task-row-degrades-to-text — flat live-query rows backed by
//! task blocks render a `state_toggle` in their own row scope
//! @pbt slips-if-removed the Now list silently degrades every task to a bare
//! h1 text blob and the keystone stays green because its seeds never render
//! a flat cross-vault task query

use std::time::Duration;
use std::time::Instant;

use holon_integration_tests::pbt::frontend_slice::components::HeadlessFrontendComponent;
use holon_integration_tests::pbt::invariants::bodies::task_rows_have_state_toggle::task_tree_rows_missing_state_toggle;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::SutFocusWrite;
use holon_pbt_core::capabilities::SutRenderer;

/// Two tasks living under a DIFFERENT page than the query page, so the query
/// result set contains none of their ancestors — the flat, parentless shape
/// the vault's Now list produces.
const NOW_TASKS_ORG: &str = "#+ID: now-tasks\n* TODO now task alpha\n:PROPERTIES:\n:ID: \
                             now-task-alpha\n:END:\nAlpha body line one.\nAlpha body line two.\n* \
                             DOING now task beta\n:PROPERTIES:\n:ID: now-task-beta\n:END:\n";

/// The Now-shaped query page: a heading owning a `holon_sql` query-source
/// child (the same shape as the vault's `Now.org` `now-query::src::0`),
/// filtering via `json_extract(properties, '$.task_state')` exactly like the
/// vault query — the fork IVM resolves matview filters against base columns,
/// so `WHERE task_state ...` is rejected ("Column 'task_state' not found in
/// schema for filter") while the properties-bag extract works. The seeded
/// vault's only task blocks are the two above, so the predicate is already
/// scoped.
const NOW_LIST_ORG: &str = "#+ID: now-list\n* Now\n:PROPERTIES:\n:ID: \
                            now-head\n:END:\n#+BEGIN_SRC holon_sql :id now-head::src::0\nSELECT \
                            b.* FROM block b WHERE COALESCE(json_extract(b.properties, \
                            '$.task_state'), '') <> ''\n#+END_SRC\n";

const TASK_IDS: [&str; 2] = ["block:now-task-alpha", "block:now-task-beta"];

#[tokio::test(flavor = "multi_thread")]
async fn flat_query_task_rows_render_a_state_toggle() {
    let comp = HeadlessFrontendComponent::new_with_loro(
        &[
            ("now-tasks.org", NOW_TASKS_ORG),
            ("now-list.org", NOW_LIST_ORG),
        ],
        Duration::from_millis(500),
        true,
    )
    .await;

    // Focus the query page the way a user does: a left-sidebar click through
    // the production driver.
    comp.apply_navigate_focus(CapRegion::Main, &EntityUri::block("now-list"))
        .await;

    // DELIVERY first, so a query/watch failure cannot be mistaken for the
    // render failure this rung guards: wait until the panel renders a
    // tree_item row for BOTH seeded tasks.
    let deadline = Instant::now() + Duration::from_secs(20);
    let snap = loop {
        let snap = comp.widget_tree_snapshot().await;
        let rendered_task_rows = snap
            .walk()
            .filter(|n| {
                n.kind == "tree_item"
                    && n.entity_id
                        .as_deref()
                        .is_some_and(|id| TASK_IDS.contains(&id))
            })
            .count();
        if rendered_task_rows == TASK_IDS.len() {
            break snap;
        }
        if Instant::now() >= deadline {
            // Split delivery from mounting: the query block's OWN resolved
            // render (watch snapshot + interpret) shows whether the task rows
            // were ever delivered to its watch.
            let head_tree = comp
                .render_tree_of(&EntityUri::parse("block:now-head").expect("now-head id"))
                .await;
            panic!(
                "precondition: the Now query page must RENDER a row per matching task. It \
                 rendered {rendered_task_rows} of {}. That is a delivery/render-mount defect, not \
                 the row-shape defect this rung guards. Rendered tree kinds: {:?}\n--- \
                 render_tree_of(now-head): {head_tree:?}",
                TASK_IDS.len(),
                snap.walk()
                    .map(|n| (n.kind.clone(), n.entity_id.clone()))
                    .filter(|(_, id)| id.is_some())
                    .collect::<Vec<_>>(),
            );
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    // The property: each task-backed row carries a state_toggle in its OWN
    // row scope — the SAME core check `inv-viewmodel-task-rows-have-state-toggle`
    // runs in the composed keystone.
    // Title-preservation guard (the other half of the rule swap): the focused
    // page's OWN root row must KEEP its page-title rendering — the page_title
    // variant renders text only, so its row scope carries NO state_toggle.
    // If the context-root rule stopped firing, the page row would fall to the
    // `default` variant, which renders a state_toggle for every block.
    let now_list_rows = snap
        .walk()
        .filter(|n| n.kind == "tree_item" && n.entity_id.as_deref() == Some("block:now-list"))
        .count();
    let toggle_free_now_list_rows =
        task_tree_rows_missing_state_toggle(&snap, &|id: &str| id == "block:now-list");
    assert!(
        now_list_rows > 0 && toggle_free_now_list_rows.len() == now_list_rows,
        "the focused page's own row(s) must render as a page TITLE (no state_toggle in the row \
         scope). {} of {now_list_rows} now-list rows are title-form; a toggle there means the \
         context-root page_title rule stopped firing and the outline lost its title treatment",
        toggle_free_now_list_rows.len(),
    );

    let missing = task_tree_rows_missing_state_toggle(&snap, &|id: &str| TASK_IDS.contains(&id));
    assert!(
        missing.is_empty(),
        "{} of {} flat-query task rows render NO state_toggle in their own row scope: \
         {missing:?}. The row degraded to a bare text/title rendering — the collection \
         tree_view's level-0 rule stamped `role: \"page_title\"` on a parentless result row. \
         Bugfunnel 2026-08-25-flat-query-task-rows-render-as-page-title-blobs.",
        missing.len(),
        TASK_IDS.len(),
    );
}
