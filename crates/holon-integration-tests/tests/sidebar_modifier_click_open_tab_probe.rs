#![cfg(feature = "pbt")]
//! **Rung-2 probes: does the cmd/ctrl-click open-in-tab path exist end to
//! end?**
//!
//! Two independent questions, deliberately answered BEFORE any reference-model
//! work (plan `~/.claude/plans/modifier-click-generation-plan.md`, I1a/I1b):
//!
//! * **I1a** — does the left sidebar's rendered row actually carry the
//!   `cmd_action` / `ctrl_action` wiring at the row's entity id, so a
//!   modifier-keyed click lookup can reach it? (`assets/default/index.org`
//!   declares `cmd_action: navigation_open_tab(...)` on the tree's
//!   `item_template`.) If not, the whole rung is moot and the defect is in
//!   `selectable` / `tree` arg plumbing, not in the model.
//! * **I1b** — when a region has TWO open `navigation_history` rows, does the
//!   main panel RENDER two subtrees, or only the cursor's? The reference-model
//!   shape for rung 2 depends entirely on the answer, so this probe drives
//!   `navigation.open_tab` DIRECTLY (no clicks, no transitions) and reports
//!   what the panel does.
//!
//! Both are keepers: they document panel/wiring semantics that nothing else
//! states executably.
//!
//! @pbt kind harness
//! @pbt covers modifier-click-open-tab — sidebar cmd/ctrl wiring + multi-root
//! panel

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::ClickModifiers;
use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::view_model::ViewModel;
use holon_integration_tests::TestEnvironment;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap(),
    )
}

/// The main panel's layout root. Mirrors `focus_path::find_region_panel`'s
/// table, which is private to `holon-frontend`.
const MAIN_PANEL: &str = "block:default-main-panel";

fn find_panel<'a>(node: &'a ViewModel, panel_id: &EntityUri) -> Option<&'a ViewModel> {
    if node.entity_id().as_ref() == Some(panel_id) {
        return Some(node);
    }
    node.children().iter().find_map(|c| find_panel(c, panel_id))
}

/// Entity ids rendered under `node`, in DFS order, deduped by first
/// appearance. Order is the assertion vehicle for "tabs render in
/// `navigation_history.id ASC` order".
fn rendered_ids(node: &ViewModel) -> Vec<EntityUri> {
    fn walk(node: &ViewModel, out: &mut Vec<EntityUri>) {
        if let Some(id) = node.entity_id() {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        for child in node.children() {
            walk(child, out);
        }
    }
    let mut out = Vec::new();
    walk(node, &mut out);
    out
}

async fn open_tab(env: &TestEnvironment, block_id: &EntityUri) {
    let mut params = HashMap::new();
    params.insert("region".to_string(), Value::String("main".to_string()));
    params.insert("block_id".to_string(), Value::String(block_id.to_string()));
    env.execute_operation("navigation", "open_tab", params)
        .await
        .unwrap_or_else(|e| panic!("navigation.open_tab({block_id}) failed: {e:#}"));
}

/// Open (non-closed) main-region history rows, `id ASC` — the prod-side truth
/// the panel is judged against.
async fn open_main_rows(env: &TestEnvironment) -> Vec<String> {
    let rows = env
        .query_sql(
            "SELECT block_id FROM navigation_history WHERE region = 'main' AND closed_at IS NULL \
             ORDER BY id ASC",
        )
        .await
        .expect("navigation_history query");
    rows.iter()
        .filter_map(|r| {
            r.get("block_id")
                .and_then(|v| v.as_string().map(String::from))
        })
        .collect()
}

// ── I1a ────────────────────────────────────────────────────────────────────

#[test]
fn sidebar_row_binds_cmd_and_ctrl_open_tab_intents() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = TestEnvironment::new_running(rt.clone())
            .await
            .expect("start a running Turso environment");
        let page = env
            .create_document("probe_tab_a.org")
            .await
            .expect("create probe_tab_a.org");

        let root_uri = holon_api::root_layout_block_uri();
        let reactive = env
            .reactive_engine
            .get()
            .expect("start_app must resolve a ReactiveEngine")
            .clone();

        // The sidebar page list is a nested `live_block` watch; its rows and
        // their bound intents stream in asynchronously (same race
        // `await_sidebar_nav_intent` guards in the PBT slice).
        let deadline = Instant::now() + Duration::from_secs(10);
        let cmd_intent = loop {
            let resolved = reactive.snapshot_resolved(&root_uri);
            if let Some(intent) = holon_frontend::focus_path::find_click_intent_in_region(
                &resolved,
                &page,
                "left_sidebar",
                ClickModifiers::cmd(),
            ) {
                break intent;
            }
            assert!(
                Instant::now() < deadline,
                "[I1a] the LeftSidebar never bound a cmd-click intent for {page} within 10s. \
                 assets/default/index.org declares `cmd_action: navigation_open_tab(...)` on the \
                 tree item_template — if the row renders but binds nothing, the defect is in \
                 selectable/tree template-arg plumbing, and rung 2's model work is moot."
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        };

        assert_eq!(
            cmd_intent.entity_name.as_str(),
            "navigation",
            "[I1a] cmd-click must dispatch a navigation op, got {cmd_intent:?}"
        );
        assert_eq!(
            cmd_intent.op_name, "open_tab",
            "[I1a] cmd-click must dispatch open_tab, got {cmd_intent:?}"
        );
        assert_eq!(
            cmd_intent.params.get("region"),
            Some(&Value::String("main".to_string())),
            "[I1a] cmd-click open_tab must target region main, got {:?}",
            cmd_intent.params
        );
        assert_eq!(
            cmd_intent.params.get("block_id"),
            Some(&Value::String(page.to_string())),
            "[I1a] cmd-click open_tab must carry the row's own id, got {:?}",
            cmd_intent.params
        );

        let resolved = reactive.snapshot_resolved(&root_uri);
        let ctrl_intent = holon_frontend::focus_path::find_click_intent_in_region(
            &resolved,
            &page,
            "left_sidebar",
            ClickModifiers::ctrl(),
        )
        .expect("[I1a] ctrl-click must bind the same open_tab wiring (Windows/Linux parity)");
        assert_eq!(ctrl_intent.op_name, "open_tab");
        assert_eq!(ctrl_intent.params, cmd_intent.params);

        // Discrimination: the primary click must still be `navigation.focus`,
        // NOT open_tab. This is what the rung-1 modifier-keyed lookup bought.
        let plain = holon_frontend::focus_path::find_click_intent_in_region(
            &resolved,
            &page,
            "left_sidebar",
            ClickModifiers::none(),
        )
        .expect("[I1a] a plain sidebar click must still bind navigation.focus");
        assert_eq!(
            plain.op_name, "focus",
            "[I1a] primary click must stay Replace-semantics focus, got {plain:?}"
        );
    });
}

// ── I1b ────────────────────────────────────────────────────────────────────

#[test]
fn many_open_history_rows_render_only_the_cursor_subtree() {
    let rt = runtime();
    rt.clone().block_on(async move {
        let env = TestEnvironment::new_running(rt.clone())
            .await
            .expect("start a running Turso environment");

        let page_a = env
            .create_document("probe_tab_a.org")
            .await
            .expect("create probe_tab_a.org");
        let page_b = env
            .create_document("probe_tab_b.org")
            .await
            .expect("create probe_tab_b.org");

        // A distinctive child under each page: the panel renders a focus root's
        // SUBTREE, so children are the robust evidence that a root is live.
        env.create_block("block:probe-child-a", page_a.as_str(), "child of A")
            .await
            .expect("create child A");
        env.create_block("block:probe-child-b", page_b.as_str(), "child of B")
            .await
            .expect("create child B");
        env.wait_for_cdc_quiescent(Duration::from_millis(200), Duration::from_secs(5))
            .await;

        open_tab(&env, &page_a).await;
        open_tab(&env, &page_b).await;
        env.wait_for_cdc_quiescent(Duration::from_millis(200), Duration::from_secs(5))
            .await;

        // Prod-side truth first. `open_tab` inserts WITHOUT closing, so the
        // boot-default focus row (`block:journals`) survives ahead of the two
        // new tabs — a region's open set is already non-singleton at boot, and
        // the reference model must not assume otherwise.
        let rows = open_main_rows(&env).await;
        assert_eq!(
            rows.iter().rev().take(2).rev().cloned().collect::<Vec<_>>(),
            vec![page_a.to_string(), page_b.to_string()],
            "[I1b] the two open_tab calls must APPEND two open main rows in id ASC order \
             (insert without closing the others). Got {rows:?} — if this is already wrong, the \
             panel question below is moot and prod open_tab is the defect."
        );

        let root_uri = holon_api::root_layout_block_uri();
        let reactive = env
            .reactive_engine
            .get()
            .expect("start_app must resolve a ReactiveEngine")
            .clone();

        // The cursor: `open_tab` moves it to the row it opened, so it sits on
        // the LAST tab opened.
        let cursor_block = env
            .query_sql(
                "SELECT nh.block_id FROM navigation_cursor nc JOIN navigation_history nh ON \
                 nh.id = nc.history_id WHERE nc.region = 'main'",
            )
            .await
            .expect("navigation_cursor query")
            .first()
            .and_then(|r| {
                r.get("block_id")
                    .and_then(|v| v.as_string().map(String::from))
            })
            .expect("main region must have a cursor on an open history row");
        assert_eq!(
            cursor_block,
            page_b.to_string(),
            "[I1b] open_tab must leave the cursor on the row it just opened"
        );

        // The panel re-projects through the focus_roots matview chain; poll
        // until the cursor's subtree is up rather than racing the snapshot.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut ids: Vec<EntityUri> = Vec::new();
        loop {
            let resolved = reactive.snapshot_resolved(&root_uri);
            let panel_id = EntityUri::parse(MAIN_PANEL).expect("static panel key");
            if let Some(panel) = find_panel(&resolved, &panel_id) {
                ids = rendered_ids(panel);
                if ids.iter().any(|i| i.as_str() == "block:probe-child-b") {
                    break;
                }
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }

        let has_a = ids.iter().any(|i| i.as_str() == "block:probe-child-a");
        let has_b = ids.iter().any(|i| i.as_str() == "block:probe-child-b");

        // THE ANSWER. Three history rows are open in `main`, yet the panel
        // renders exactly ONE subtree — the cursor's. This is by construction,
        // not a race: the main panel's query
        // (`assets/default/index.org`, the `default-main-panel::src::0` block)
        // ends with `JOIN navigation_cursor nc ON nc.region = fr.region AND
        // nc.history_id = fr.history_id`, so open-but-not-current rows are
        // filtered out. Tabs are BACKGROUND rows, not a split view.
        assert!(
            has_b,
            "[I1b] the panel must render the CURSOR's subtree (child of {page_b}). \
             Panel ids: {ids:?}"
        );
        assert!(
            !has_a,
            "[I1b] the panel must NOT render a non-cursor open tab's subtree — the main-panel \
             query joins navigation_cursor, so only the current row projects. If child A now \
             renders, the panel became multi-root and the reference model for tabs must change \
             shape accordingly. Panel ids: {ids:?}"
        );
    });
}
