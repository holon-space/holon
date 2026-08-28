//! **`add_subtask` must write the `tags` and `requires` edge fields.**
//!
//! `add_subtask` is how an agent creates work items over MCP. The G1 now-query
//! pivots on the `requires` edge (a task is unblocked when every `required_id`
//! is DONE) and on the `agent` tag, so a create that drops those two params
//! leaves the agent unable to author its own queue — it can mint a task but
//! never place it in the dependency graph.
//!
//! Drives the real tool over the wire against the composed `full_headless`
//! session, then reads the junction tables the SQL provider's edge partition
//! owns (`block_tags`, `block_requires`) — the storage the now-query joins, not
//! a restatement of the tool's own response.
//!
//! `block_requires.required_id` carries NO foreign key: the target column is
//! deliberately unconstrained so a forward or cross-file `:REQUIRES:` edge
//! cannot abort a whole org ingest (see the rationale comment in
//! `crates/holon-turso/sql/schema/block_requires.sql` and the migration that
//! strips the old target FK, `crates/holon-turso/src/schema_modules.rs:1398`).
//! Storage therefore accepts a dangling target and
//! `block_requirement_edges_matview` INNER-JOINs it away — silently. That is
//! right for bulk ingest and wrong for an interactive agent tool, so
//! `add_subtask` validates target existence at ITS boundary and refuses the
//! whole create. The last two rungs pin that boundary.
//!
//! Requires `--features pbt`.
//!
//! @pbt kind harness
//! @pbt covers mcp-add-subtask-edges — `tags`/`requires` supplied to
//! `add_subtask` reach their junction tables, and unresolvable or
//! foreign-scheme `requires` targets are refused whole

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_integration_tests::McpUserDriver;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::transitions::WriteOrgFile;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

type Sut = ComposedSut<WideE2E>;

/// The generator's placeholder document uri: top-level headings are parented to
/// it and `apply_to_ref` remaps them to the resolved per-document uri.
fn gen_placeholder() -> EntityUri {
    EntityUri::block("gen-placeholder")
}

fn heading(id: &str, headline: &str) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), gen_placeholder(), headline);
    b.set_property("ID", Value::String(id.to_string()));
    b
}

/// A parent to append under and a blocker to point `requires` at. The blocker
/// is a REAL block so the happy-path rung and the dangling-target rung differ
/// in exactly one thing — whether the target resolves.
fn page_file() -> WriteOrgFile {
    WriteOrgFile {
        filename: "add_subtask_edges.org".to_string(),
        blocks: vec![
            heading("as-parent", "Parent heading"),
            heading("as-blocker", "TODO Blocking work"),
        ],
        keyword_set: None,
    }
}

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

#[test]
fn mcp_add_subtask_writes_tags_and_requires_edges() {
    holon_integration_tests::pbt::set_loro_peer_id_if_unset("1");
    let ref_state = wide_e2e_ref();
    let sut = Sut::init_test(&ref_state);
    let (ref_state, sut) = step(ref_state, sut, E2ETransition::WriteOrgFile(page_file()));

    ref_state
        .doc_uri_by_name("add_subtask_edges")
        .expect("add_subtask_edges page block must exist after WriteOrgFile");
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
    let frontend = sut
        .handle()
        .frontend()
        .expect("full_headless boots a frontend component")
        .clone();

    let port = free_loopback_port();
    // The embedded server reads MCP_SERVER_PORT (default_port is only a
    // fallback) — pin it so the listener lands where the driver connects.
    unsafe {
        std::env::set_var("MCP_SERVER_PORT", port.to_string());
    }
    let debug = sut.runtime().block_on(frontend.mcp_debug_services());
    {
        let _guard = sut.runtime().enter();
        let services: std::sync::Arc<dyn holon_frontend::reactive::BuilderServices> =
            reactive.clone();
        holon_mcp::di::start_embedded_mcp_server_with_debug(
            Some(engine.clone()),
            Some(services),
            port,
            debug,
        );
    }
    holon_integration_tests::pbt::wait_for_mcp_listener(
        port,
        holon_integration_tests::pbt::ui_harness::MCP_BIND_TIMEOUT,
        "mcp-add-subtask-edges",
    );

    let base_url = format!("http://127.0.0.1:{port}/mcp");
    let driver = sut
        .runtime()
        .block_on(McpUserDriver::connect(&base_url))
        .expect("connect to the embedded holon MCP server");

    // Bare ids on both params — the agent-facing shape. `ensure_block_prefix`
    // is the boundary that schemes them.
    let created = sut
        .runtime()
        .block_on(driver.call_tool_json(
            "add_subtask",
            serde_json::json!({
                "parent_id": "as-parent",
                "title": "TODO Wire the edges",
                "tags": ["agent", "wiring"],
                "requires": ["as-blocker"],
            }),
        ))
        .expect("add_subtask over MCP");
    let new_id = created["task_id"]
        .as_str()
        .unwrap_or_else(|| panic!("add_subtask must return a task_id; got {created}"))
        .to_string();
    sut.settle_projections();

    assert_eq!(
        tags_of(&sut, &driver, &new_id),
        vec!["agent".to_string(), "wiring".to_string()],
        "add_subtask's `tags` must reach block_tags for {new_id} as EXACTLY that set — a \
         duplicate or an extra tag is a bug, not a pass"
    );
    assert_eq!(
        requires_of(&sut, &driver, &new_id),
        vec!["block:as-blocker".to_string()],
        "add_subtask's `requires` must reach block_requires for {new_id} as EXACTLY that set, \
         bare id normalized at the boundary"
    );
    assert_eq!(
        created["tags"],
        serde_json::json!(["agent", "wiring"]),
        "add_subtask must echo the `tags` it wrote; got {created}"
    );
    assert_eq!(
        created["requires"],
        serde_json::json!(["block:as-blocker"]),
        "add_subtask must echo `requires` as WRITTEN, so the caller sees the boundary \
         normalization of its bare id; got {created}"
    );

    // ── Rung 2: neither edge param supplied — the overwhelmingly common call.
    //    Succeeds, writes no junction rows, and echoes empty sets. ──
    let plain = sut
        .runtime()
        .block_on(driver.call_tool_json(
            "add_subtask",
            serde_json::json!({ "parent_id": "as-parent", "title": "TODO Plain subtask" }),
        ))
        .expect("add_subtask with no edge params over MCP");
    let plain_id = plain["task_id"]
        .as_str()
        .unwrap_or_else(|| panic!("add_subtask must return a task_id; got {plain}"))
        .to_string();
    sut.settle_projections();
    assert!(
        tags_of(&sut, &driver, &plain_id).is_empty()
            && requires_of(&sut, &driver, &plain_id).is_empty(),
        "an add_subtask with no edge params must leave both junctions empty for {plain_id}"
    );
    assert_eq!(
        (&plain["tags"], &plain["requires"]),
        (&serde_json::json!([]), &serde_json::json!([])),
        "the echo must report empty edge sets rather than omitting the keys; got {plain}"
    );

    // ── Rung 3: an unresolvable `requires` target. Storage would accept it and
    //    the matview would INNER-JOIN it away, so the task would be silently
    //    never blocked — the tool must refuse the whole create instead. ──
    let dangling = sut
        .runtime()
        .block_on(driver.call_tool_json(
            "add_subtask",
            serde_json::json!({
                "parent_id": "as-parent",
                "title": "TODO Dangling target subtask",
                "requires": ["as-ghost"],
            }),
        ))
        .expect_err(
            "add_subtask must REFUSE an unresolvable `requires` target — storage has no target \
             FK, so accepting it silently drops the dependency at the matview join",
        );
    let dangling_msg = format!("{dangling:#}");
    assert!(
        dangling_msg.contains("as-ghost"),
        "the refusal must NAME the unresolvable target so the agent can fix its call; got \
         {dangling_msg}"
    );
    assert_eq!(
        blocks_titled(&sut, &driver, "Dangling target subtask"),
        0,
        "a refused add_subtask must create NOTHING — no half-born block without its dependency"
    );

    // ── Rung 4: a foreign scheme. `ensure_block_prefix` only tests for a
    //    `block:` prefix, so `doc:notes` would be stored as `block:doc:notes`
    //    and echoed back as though it had been normalized. ──
    let scheme_err = sut
        .runtime()
        .block_on(driver.call_tool_json(
            "add_subtask",
            serde_json::json!({
                "parent_id": "as-parent",
                "title": "TODO Foreign scheme subtask",
                "requires": ["doc:notes"],
            }),
        ))
        .expect_err("add_subtask must REFUSE a non-`block:` scheme in `requires`");
    let scheme_msg = format!("{scheme_err:#}");
    assert!(
        scheme_msg.contains("doc:notes") && scheme_msg.contains("scheme"),
        "the refusal must name the offending id AND identify it as a SCHEME error. A plain \
         does-not-exist error would pass a weaker assertion while meaning `doc:notes` had been \
         blindly prefixed to `block:doc:notes` and merely failed to resolve; got {scheme_msg}"
    );
    assert_eq!(
        blocks_titled(&sut, &driver, "Foreign scheme subtask"),
        0,
        "a refused add_subtask must create NOTHING"
    );
    let corrupted = sut
        .runtime()
        .block_on(driver.execute_raw_sql(
            "SELECT required_id FROM block_requires WHERE required_id LIKE 'block:doc:%'",
        ))
        .expect("execute_raw_sql double-scheme probe over MCP");
    assert_eq!(
        corrupted["row_count"], 0,
        "no `block:doc:…` double-schemed target may ever reach block_requires; got {corrupted}"
    );

    // ── Rung 5: the same target — and the same tag — supplied twice. Both
    //    junctions are keyed `PRIMARY KEY (block_id, <target>)` and the
    //    provider emits one PLAIN `INSERT` per element, so a repeat collides
    //    with the row it just wrote. A caller naming a blocker twice means it
    //    once, so this is deduped rather than refused. ──
    let duped = sut
        .runtime()
        .block_on(driver.call_tool_json(
            "add_subtask",
            serde_json::json!({
                "parent_id": "as-parent",
                "title": "TODO Duplicate target subtask",
                "tags": ["agent", "agent"],
                "requires": ["as-blocker", "as-blocker"],
            }),
        ))
        .expect(
            "a repeated `requires`/`tags` entry is unambiguous caller intent and must be deduped, \
             not collide with the junction primary key",
        );
    let duped_id = duped["task_id"]
        .as_str()
        .unwrap_or_else(|| panic!("add_subtask must return a task_id; got {duped}"))
        .to_string();
    sut.settle_projections();
    assert_eq!(
        requires_of(&sut, &driver, &duped_id),
        vec!["block:as-blocker".to_string()],
        "a target named twice must land as ONE junction row"
    );
    assert_eq!(
        tags_of(&sut, &driver, &duped_id),
        vec!["agent".to_string()],
        "a tag named twice must land as ONE junction row"
    );
    assert_eq!(
        (&duped["tags"], &duped["requires"]),
        (
            &serde_json::json!(["agent"]),
            &serde_json::json!(["block:as-blocker"])
        ),
        "the echo must report the deduped sets that were actually stored; got {duped}"
    );
    assert_eq!(
        blocks_titled(&sut, &driver, "Duplicate target subtask"),
        1,
        "exactly one block — a create that half-succeeded would leave the block row behind"
    );
}

/// The single string column of every returned row, in query order.
fn column(rows: &serde_json::Value, name: &str) -> Vec<String> {
    rows["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("query response must carry a `rows` array; got {rows}"))
        .iter()
        .map(|r| {
            r[name]
                .as_str()
                .unwrap_or_else(|| panic!("row is missing string column {name:?}; got {r}"))
                .to_string()
        })
        .collect()
}

fn tags_of(sut: &Sut, driver: &McpUserDriver, block_id: &str) -> Vec<String> {
    let rows = sut
        .runtime()
        .block_on(driver.execute_raw_sql(&format!(
            "SELECT tag FROM block_tags WHERE block_id = '{block_id}' ORDER BY tag"
        )))
        .expect("execute_raw_sql block_tags over MCP");
    column(&rows, "tag")
}

fn requires_of(sut: &Sut, driver: &McpUserDriver, block_id: &str) -> Vec<String> {
    let rows = sut
        .runtime()
        .block_on(driver.execute_raw_sql(&format!(
            "SELECT required_id FROM block_requires WHERE block_id = '{block_id}' ORDER BY \
             required_id"
        )))
        .expect("execute_raw_sql block_requires over MCP");
    column(&rows, "required_id")
}

/// How many blocks carry `title` in their content — the "was anything created?"
/// probe for the refusal rungs.
fn blocks_titled(sut: &Sut, driver: &McpUserDriver, title: &str) -> u64 {
    let rows = sut
        .runtime()
        .block_on(driver.execute_raw_sql(&format!(
            "SELECT id FROM block_raw WHERE content LIKE '%{title}%'"
        )))
        .expect("execute_raw_sql block_raw title probe over MCP");
    rows["row_count"]
        .as_u64()
        .unwrap_or_else(|| panic!("query response must carry a numeric `row_count`; got {rows}"))
}
