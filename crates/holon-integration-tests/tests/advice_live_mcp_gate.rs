//! **Increment F step 7 — the live-instance MCP gate** (advice feature; ADR
//! 0021/0022, docs/Proposals/advice-feature-implementation-plan.md).
//!
//! The plan's step 7 is the ONE part of the advice loop that is deliberately
//! NOT a keystone red-green iteration: "the final proof is an end-to-end run
//! via the `holon` MCP on a running instance — lesson surfaces under its task,
//! dismissal removes it and persists, deletion leaves no dangling row." It is
//! the integration check the headless keystone (`advice_step4_red.rs`) cannot
//! cover, because that test asserts on the in-process render snapshot, not on
//! the MCP surface a real frontend exposes.
//!
//! This gate makes that run self-contained and CI-green: it boots the SAME
//! composed `full_headless` session the keystone drives (a real Turso
//! `BackendEngine` with the advice-rule reconciler installed + a real
//! `ReactiveEngine` carrying the advice weave sidecar), seeds the exact advice
//! scenario the step-6 proof uses, then attaches the embedded `holon` MCP
//! server (`holon_mcp::di::start_embedded_mcp_server_with_debug` — the same
//! server every frontend launches) over that session and drives EVERY
//! assertion OVER THE MCP WIRE:
//!
//! 1. `describe_ui` shows the lessons woven under the task (they live on a
//!    separate page, so their appearance on the anchor's page is proof of the
//!    weave, and each carries the read-only `dismiss_advice` affordance).
//! 2. `execute_operation block.dismiss_advice` (the wire form of the frontend
//!    op_button) removes the top lesson, the 3rd candidate backfills at k=2,
//!    and `execute_raw_sql` confirms the dismissal PERSISTED in
//!    `advice_suppressed`.
//! 3. `execute_operation block.delete` on a woven lesson leaves NO dangling row
//!    in the rule's anchor-denormalized matview.
//!
//! Mirrors the `McpUserDriver` rung (`mcp_user_driver.rs`) and the
//! `try_start_embedded_mcp` helper (`pbt/ui_harness.rs`), but backs the server
//! with an in-process composed session rather than an external app, so no live
//! app / iOS sim is required. Requires `--features pbt`.

use holon_api::EdgeFieldUpdate;
use holon_api::EntityUri;
use holon_api::Region;
use holon_api::Tags;
use holon_api::Value;
use holon_api::block::Block;
use holon_integration_tests::McpUserDriver;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::transitions::NavigateFocus;
use holon_integration_tests::pbt::transitions::SetEdgeField;
use holon_integration_tests::pbt::transitions::WriteOrgFile;
use holon_orgmode::models::OrgBlockExt;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

/// The generator's placeholder document uri (`WriteOrgFile::GEN_PLACEHOLDER`):
/// top-level headings are parented to it and `apply_to_ref` remaps them to the
/// resolved per-document uri. Mirrors `advice_step4_red.rs`.
fn gen_placeholder() -> EntityUri {
    EntityUri::block("gen-placeholder")
}

fn heading(id: &str, headline: &str) -> Block {
    let uri = EntityUri::block(id);
    let mut b = Block::new_text(uri, gen_placeholder(), headline);
    b.set_property("ID", Value::String(id.to_string()));
    b
}

/// The advice-rule file — one ACTIVE `holon_advice_rule_yaml` block: anchors
/// are `task`-tagged blocks, candidates are `lesson`-tagged blocks sharing a
/// tag, ranked by shared-tag count, `k = 2` (truncation live). Same shape the
/// step-6 proof and the bundled `lessons_for_tasks.yaml` mint.
fn advice_rule_file() -> WriteOrgFile {
    let rule_heading = heading("advice-rule-h", "Advice rules");
    let yaml = concat!(
        "name: pbt_lessons\n",
        "active: true\n",
        "anchor:\n",
        "  has_tag: task\n",
        "candidates:\n",
        "  tag_overlap_recency:\n",
        "    source:\n",
        "      has_tag: lesson\n",
        "k: 2\n",
    );
    let parsed = holon_advice::parse_advice_rule(yaml).expect("hand-built advice rule must parse");
    assert!(parsed.active);
    assert_eq!(parsed.k.get(), 2);

    let mut rule_block = Block::new_source(
        EntityUri::block("advice-rule-h::src::0"),
        EntityUri::block("advice-rule-h"),
        "holon_advice_rule_yaml",
        yaml,
    );
    rule_block.set_sequence(1);
    WriteOrgFile {
        filename: "advice_1.org".to_string(),
        blocks: vec![rule_heading, rule_block],
        keyword_set: None,
    }
}

/// The task anchor — on its OWN page, so nothing but the weave can put a lesson
/// on it.
fn anchor_file() -> WriteOrgFile {
    WriteOrgFile {
        filename: "anchor_1.org".to_string(),
        blocks: vec![heading("anchor-a", "Ship the release")],
        keyword_set: None,
    }
}

/// Three lessons on a SEPARATE page (never navigated), so their canonical home
/// is elsewhere and their appearance under the anchor is unambiguously the
/// weave. k=2 keeps the 3rd (`lesson-d`) as the live truncation / backfill
/// candidate.
fn lessons_file() -> WriteOrgFile {
    WriteOrgFile {
        filename: "lessons_1.org".to_string(),
        blocks: vec![
            heading("lesson-b", "Cut a changelog first"),
            heading("lesson-c", "Tag the commit"),
            heading("lesson-d", "Announce in the channel"),
        ],
        keyword_set: None,
    }
}

fn set_tags(id: &str, csv: &str) -> SetEdgeField {
    SetEdgeField {
        block_id: EntityUri::block(id),
        update: EdgeFieldUpdate::Tags(Tags::from_csv(csv)),
    }
}

type Sut = ComposedSut<WideE2E>;

/// Precondition-checked apply through the keystone's exact path (mirrors
/// `advice_step4_red::step`).
fn step(mut ref_state: ReferenceState, sut: Sut, t: E2ETransition) -> (ReferenceState, Sut) {
    assert!(
        WideE2EMachine::preconditions(&ref_state, &t),
        "preconditions failed for {t:?}"
    );
    ref_state = WideE2EMachine::apply(ref_state, &t);
    let sut = <Sut as StateMachineTest>::apply(sut, &ref_state, t);
    (ref_state, sut)
}

/// Bind an ephemeral loopback port, then release it — the embedded MCP server
/// re-binds it. A tiny TOCTOU window, acceptable for a single-process test.
fn free_loopback_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

/// `describe_ui` for the main panel as a JSON tree.
async fn main_panel_ui(driver: &McpUserDriver) -> serde_json::Value {
    driver
        .call_tool_json(
            "describe_ui",
            serde_json::json!({ "block_id": MAIN_PANEL, "format": "json" }),
        )
        .await
        .expect("describe_ui over MCP")
}

/// Collect the `entity.id` of every widget node in a `describe_ui` tree.
///
/// A woven advice row is a node whose entity ROW id is the canonical lesson id
/// (ADR 0015 rule 3: the display-placed row keeps the lesson's id; the
/// `Occurrence` is `serde(skip)` so it never rides the wire). This must key on
/// a node's entity id — NOT a raw substring over the JSON — because the anchor
/// node's own row carries an `advice_suppressed` COLUMN naming every dismissed
/// lesson: after a dismissal the anchor's row literally contains the dismissed
/// lesson id, which is expected state, not a surviving woven row.
fn collect_entity_ids(v: &serde_json::Value, out: &mut std::collections::BTreeSet<String>) {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Object(entity)) = map.get("entity") {
                if let Some(serde_json::Value::String(id)) = entity.get("id") {
                    out.insert(id.clone());
                }
            }
            for val in map.values() {
                collect_entity_ids(val, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr {
                collect_entity_ids(val, out);
            }
        }
        _ => {}
    }
}

/// The set of widget-node entity ids in the current main-panel tree.
fn rendered_entity_ids(ui: &serde_json::Value) -> std::collections::BTreeSet<String> {
    let mut ids = std::collections::BTreeSet::new();
    collect_entity_ids(ui, &mut ids);
    ids
}

const MAIN_PANEL: &str = "block:default-main-panel";

#[test]
fn advice_live_mcp_gate() {
    holon_integration_tests::pbt::set_loro_peer_id_if_unset("1");
    let ref_state = wide_e2e_ref();
    assert!(
        ref_state.action.app_started,
        "wide_e2e_ref boots pre-started"
    );
    let sut = Sut::init_test(&ref_state);

    // ── Seed the advice scenario through the keystone transition path (proven
    //    green by advice_step4_red): ACTIVE rule, anchor on its own page, three
    //    lessons on another page, navigate the main panel to the anchor page,
    //    then pool-tag so b∩a=3, c∩a=2, d∩a=1 → k=2 woven {b,c}, d truncated. ──
    let (ref_state, sut) = step(
        ref_state,
        sut,
        E2ETransition::WriteOrgFile(advice_rule_file()),
    );
    let (ref_state, sut) = step(ref_state, sut, E2ETransition::WriteOrgFile(anchor_file()));
    let (ref_state, sut) = step(ref_state, sut, E2ETransition::WriteOrgFile(lessons_file()));

    let anchor_doc = ref_state
        .doc_uri_by_name("anchor_1")
        .expect("anchor_1 page block must exist after WriteOrgFile");
    let (ref_state, sut) = step(
        ref_state,
        sut,
        E2ETransition::NavigateFocus(NavigateFocus {
            region: Region::Main,
            block_id: anchor_doc,
        }),
    );

    let (ref_state, sut) = step(
        ref_state,
        sut,
        E2ETransition::SetEdgeField(set_tags("anchor-a", "task,p1,p2,p3")),
    );
    let (ref_state, sut) = step(
        ref_state,
        sut,
        E2ETransition::SetEdgeField(set_tags("lesson-b", "lesson,p1,p2,p3")),
    );
    let (ref_state, sut) = step(
        ref_state,
        sut,
        E2ETransition::SetEdgeField(set_tags("lesson-c", "lesson,p1,p2")),
    );
    let (_ref_state, sut) = step(
        ref_state,
        sut,
        E2ETransition::SetEdgeField(set_tags("lesson-d", "lesson,p1")),
    );

    // Converge the three projections + recompute the weave sidecar (the same
    // settle the WideE2E slice runs after every apply), so the render the MCP
    // server snapshots is the post-weave one.
    sut.settle_projections();
    let engine = sut
        .handle()
        .engine()
        .expect("full_headless boots a Turso engine")
        .clone();
    let reactive = sut
        .handle()
        .reactive()
        .expect("full_headless boots a ReactiveEngine");
    sut.runtime().block_on(reactive.refresh_advice_sidecar());

    // ── Attach the embedded holon MCP server over THIS session and connect a
    //    driver — every assertion below crosses the MCP wire. ──
    let port = free_loopback_port();
    // The embedded server reads MCP_SERVER_PORT (default_port is only a
    // fallback) — pin it to the port we bound so the listener lands where the
    // driver connects.
    unsafe {
        std::env::set_var("MCP_SERVER_PORT", port.to_string());
    }
    {
        let _guard = sut.runtime().enter();
        let services: std::sync::Arc<dyn holon_frontend::reactive::BuilderServices> =
            reactive.clone();
        holon_mcp::di::start_embedded_mcp_server_with_debug(
            Some(engine.clone()),
            Some(services),
            port,
            std::sync::Arc::new(holon_mcp::server::DebugServices::default()),
        );
    }
    std::thread::sleep(std::time::Duration::from_secs(2));

    let base_url = format!("http://127.0.0.1:{port}/mcp");
    let driver = sut
        .runtime()
        .block_on(McpUserDriver::connect(&base_url))
        .expect("connect to the embedded holon MCP server");

    let anchor = EntityUri::block("anchor-a");
    let lesson_b = EntityUri::block("lesson-b");
    let lesson_c = EntityUri::block("lesson-c");
    let lesson_d = EntityUri::block("lesson-d");

    // ── 1. The lessons surface woven under the task (over MCP). ──
    let ui = sut.runtime().block_on(main_panel_ui(&driver));
    let ids = rendered_entity_ids(&ui);
    let ui_str = serde_json::to_string(&ui).expect("serialize describe_ui json");
    assert!(
        ids.contains(lesson_b.as_str()),
        "describe_ui: top lesson {lesson_b} must be woven under the anchor \
         (it lives on a separate page, so a rendered row with this id is the \
         weave). rendered ids={ids:?} ui={ui_str}"
    );
    assert!(
        ids.contains(lesson_c.as_str()),
        "describe_ui: 2nd lesson {lesson_c} must be woven under the anchor. \
         rendered ids={ids:?} ui={ui_str}"
    );
    assert!(
        !ids.contains(lesson_d.as_str()),
        "describe_ui: {lesson_d} is the k=2 truncation → no woven row pre-dismiss. \
         rendered ids={ids:?} ui={ui_str}"
    );
    assert!(
        ui_str.contains("dismiss_advice"),
        "describe_ui: woven advice rows must carry the read-only dismiss_advice \
         affordance (ADR 0021). ui={ui_str}"
    );

    // ── 2. Dismissal over MCP removes the top lesson, backfills the 3rd, and
    //       persists. ──
    sut.runtime()
        .block_on(driver.call_tool_text(
            "execute_operation",
            serde_json::json!({
                "entity_name": "block",
                "operation": "dismiss_advice",
                "params": { "anchor_id": anchor.as_str(), "lesson_id": lesson_b.as_str() },
            }),
        ))
        .expect("dismiss_advice over MCP");
    sut.settle_projections();
    sut.runtime().block_on(reactive.refresh_advice_sidecar());

    let ui = sut.runtime().block_on(main_panel_ui(&driver));
    let ids = rendered_entity_ids(&ui);
    let ui_str = serde_json::to_string(&ui).expect("serialize describe_ui json");
    assert!(
        !ids.contains(lesson_b.as_str()),
        "describe_ui post-dismiss: {lesson_b}'s woven row must be gone (its id may \
         still appear in the anchor's advice_suppressed column — that is expected). \
         rendered ids={ids:?} ui={ui_str}"
    );
    assert!(
        ids.contains(lesson_c.as_str()),
        "describe_ui post-dismiss: {lesson_c} must remain woven. \
         rendered ids={ids:?} ui={ui_str}"
    );
    assert!(
        ids.contains(lesson_d.as_str()),
        "describe_ui post-dismiss: {lesson_d} must BACKFILL at k=2. \
         rendered ids={ids:?} ui={ui_str}"
    );

    // Persistence: the dismissal is durable in the authored exclusion set
    // (`advice_suppressed`), read over the wire.
    let rows = sut
        .runtime()
        .block_on(driver.execute_raw_sql("SELECT anchor_id, lesson_id FROM advice_suppressed"))
        .expect("execute_raw_sql advice_suppressed over MCP");
    let rows_str = serde_json::to_string(&rows).expect("serialize rows");
    assert!(
        rows_str.contains(anchor.as_str()) && rows_str.contains(lesson_b.as_str()),
        "advice_suppressed must persist ({anchor}, {lesson_b}); got {rows_str}"
    );

    // ── 3. Deleting a woven lesson leaves NO dangling row in the rule matview. ──
    sut.runtime()
        .block_on(driver.call_tool_text(
            "execute_operation",
            serde_json::json!({
                "entity_name": "block",
                "operation": "delete",
                "params": { "id": lesson_c.as_str() },
            }),
        ))
        .expect("delete lesson-c over MCP");
    sut.settle_projections();
    sut.runtime().block_on(reactive.refresh_advice_sidecar());

    let matview_rows = sut
        .runtime()
        .block_on(driver.execute_raw_sql("SELECT lesson_id FROM advice_rule_pbt_lessons"))
        .expect("execute_raw_sql advice matview over MCP");
    let matview_str = serde_json::to_string(&matview_rows).expect("serialize matview rows");
    assert!(
        !matview_str.contains(lesson_c.as_str()),
        "deleted lesson {lesson_c} must leave no dangling row in advice_rule_pbt_lessons; \
         got {matview_str}"
    );
    assert!(
        matview_str.contains(lesson_d.as_str()),
        "the surviving lesson {lesson_d} must still be in the matview; got {matview_str}"
    );
}
