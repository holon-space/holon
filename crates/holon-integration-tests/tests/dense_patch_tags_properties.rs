//! **`dense_patch` writes the tags and drawer properties of the rows it
//! patches.**
//!
//! An agent posts a decision — a `:decision:` block with `choose`/`recommend`
//! and option children carrying `option` — as ONE `dense_patch`. The dense
//! text is ordinary org: tags are the headline tag group, properties are the
//! `:PROPERTIES:` drawer that `dense_query` itself renders. The same holds for
//! an existing row: an edited tag group or drawer line is written back, and an
//! unedited tagged row round-trips to no write at all.
//!
//! Drives the real tool over the wire against the composed `full_headless`
//! session, then reads `block_raw`/`block_tags` and the org file on disk.
//!
//! Requires `--features pbt`.
//!
//! @pbt kind harness
//! @pbt covers dense-patch-tags-properties — tags and drawer properties in a
//!   dense row reach the store and the org file on create and on update and
//!   read back as written, drawer line order included; a drawer key naming a
//!   storage column refuses the whole patch

use holon_api::EntityUri;
use holon_api::Tags;
use holon_api::Value;
use holon_api::block::Block;
use holon_integration_tests::McpUserDriver;
use holon_integration_tests::pbt::composed::embedded_mcp::EmbeddedMcp;
use holon_integration_tests::pbt::composed::embedded_mcp::connect_embedded_mcp;
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

const FILE: &str = "dense_tags.org";

fn gen_placeholder() -> EntityUri {
    EntityUri::block("gen-placeholder")
}

fn heading(id: &str, parent: EntityUri, headline: &str, tags: &[&str]) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), parent, headline);
    b.set_property("ID", Value::String(id.to_string()));
    b.tags = Tags::from_tag_iter(tags.iter().map(|t| t.to_string()));
    b
}

fn page_file() -> WriteOrgFile {
    WriteOrgFile {
        filename: FILE.to_string(),
        blocks: vec![
            heading("dt-parent", gen_placeholder(), "Decisions home", &[]),
            heading(
                "dt-kept",
                EntityUri::block("dt-parent"),
                "Kept note",
                &["keep"],
            ),
            heading(
                "dt-edited",
                EntityUri::block("dt-parent"),
                "Edited note",
                &["old"],
            ),
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

fn connect(sut: &Sut) -> EmbeddedMcp {
    connect_embedded_mcp(sut, "dense-patch-tags-properties")
}

/// `dense_query` of the parent's children: (handle, dense text).
fn project(sut: &Sut, driver: &McpUserDriver) -> (String, String) {
    let proj = sut
        .runtime()
        .block_on(driver.call_tool_json(
            "dense_query",
            serde_json::json!({
                "query": "SELECT * FROM block WHERE parent_id = 'block:dt-parent' ORDER BY sort_key",
                "language": "holon_sql",
            }),
        ))
        .expect("dense_query over MCP");
    assert_eq!(
        proj["omitted_properties"],
        serde_json::json!({}),
        "every property here is spellable, so no row omits one: {proj}"
    );
    (
        proj["projection_handle"]
            .as_str()
            .unwrap_or_else(|| panic!("dense_query returned no handle: {proj}"))
            .to_string(),
        proj["dense_org"]
            .as_str()
            .unwrap_or_else(|| panic!("dense_query returned no dense_org: {proj}"))
            .to_string(),
    )
}

fn patch(sut: &Sut, driver: &McpUserDriver, handle: &str, text: &str) -> serde_json::Value {
    sut.runtime()
        .block_on(driver.call_tool_json(
            "dense_patch",
            serde_json::json!({ "handle": handle, "text": text }),
        ))
        .unwrap_or_else(|e| panic!("dense_patch failed: {e:#}\ntext:\n{text}"))
}

fn rows(sut: &Sut, driver: &McpUserDriver, sql: &str) -> Vec<serde_json::Value> {
    let result = sut
        .runtime()
        .block_on(driver.execute_raw_sql(sql))
        .unwrap_or_else(|e| panic!("execute_raw_sql {sql:?} failed: {e:#}"));
    result["rows"]
        .as_array()
        .unwrap_or_else(|| panic!("query response must carry `rows`; got {result}"))
        .clone()
}

/// The single block whose content is `content`: (id, properties JSON).
fn block_by_content(
    sut: &Sut,
    driver: &McpUserDriver,
    content: &str,
) -> (String, serde_json::Value) {
    let found = rows(
        sut,
        driver,
        &format!("SELECT id, properties FROM block_raw WHERE content = '{content}'"),
    );
    assert_eq!(
        found.len(),
        1,
        "exactly one block must carry content {content:?}; got {found:?}"
    );
    let id = found[0]["id"].as_str().expect("id is a string").to_string();
    let props = match &found[0]["properties"] {
        serde_json::Value::String(s) => serde_json::from_str(s).expect("properties is JSON"),
        other => other.clone(),
    };
    (id, props)
}

fn tags_of(sut: &Sut, driver: &McpUserDriver, block_id: &str) -> Vec<String> {
    rows(
        sut,
        driver,
        &format!("SELECT tag FROM block_tags WHERE block_id = '{block_id}' ORDER BY tag"),
    )
    .iter()
    .map(|r| r["tag"].as_str().expect("tag is a string").to_string())
    .collect()
}

fn org_file(sut: &Sut, driver: &McpUserDriver) -> String {
    let docs = sut
        .runtime()
        .block_on(driver.call_tool_json("list_loro_documents", serde_json::json!({})))
        .expect("list_loro_documents over MCP");
    let doc_id = docs["aliases"]
        .as_array()
        .unwrap_or_else(|| panic!("list_loro_documents has no `aliases`: {docs}"))
        .iter()
        .find(|a| a["file_path"].as_str().is_some_and(|p| p.ends_with(FILE)))
        .map(|a| a["alias"].as_str().expect("alias is a string").to_string())
        .unwrap_or_else(|| panic!("no alias for {FILE}: {docs}"));
    let disk = sut
        .runtime()
        .block_on(driver.call_tool_json("read_org_file", serde_json::json!({ "doc_id": doc_id })))
        .expect("read_org_file over MCP");
    disk["content"]
        .as_str()
        .unwrap_or_else(|| panic!("read_org_file has no `content`: {disk}"))
        .to_string()
}

const DECISION: &str = "\
* ? Which store backs the cache? :decision:
:PROPERTIES:
:choose: 1
:recommend: a
:END:
** Sled
:PROPERTIES:
:option: a
:END:
** SQLite
:PROPERTIES:
:option: b
:END:
";

#[test]
fn dense_patch_writes_tags_and_properties() {
    holon_integration_tests::pbt::set_loro_peer_id_if_unset("1");
    let ref_state = wide_e2e_ref();
    let sut = Sut::init_test(&ref_state);
    let (_ref_state, sut) = step(ref_state, sut, E2ETransition::WriteOrgFile(page_file()));
    sut.settle_projections();
    let driver = connect(&sut);

    // ── Rung 1: create a decision with two options in ONE patch, and leave
    //    the tagged existing rows untouched. ──
    let (handle, dense) = project(&sut, &driver);
    assert!(
        dense.contains("Kept note :keep: {#"),
        "dense_query renders a tagged row as `title :tag: {{#alias}}`; got:\n{dense}"
    );
    let mut edited = dense.clone();
    if !edited.ends_with('\n') {
        edited.push('\n');
    }
    edited.push_str(DECISION);
    let applied = patch(&sut, &driver, &handle, &edited);
    assert_eq!(
        (applied["created"].as_u64(), applied["updated"].as_u64()),
        (Some(3), Some(0)),
        "three creates and NO update — an unedited tagged row must not be rewritten; got \
         {applied}"
    );
    sut.settle_projections();

    let (decision, props) = block_by_content(&sut, &driver, "Which store backs the cache?");
    assert_eq!(tags_of(&sut, &driver, &decision), vec!["decision"]);
    assert_eq!(
        (props["choose"].as_str(), props["recommend"].as_str()),
        (Some("1"), Some("a")),
        "the decision's drawer properties must reach block_raw; got {props}"
    );
    for (title, key) in [("Sled", "a"), ("SQLite", "b")] {
        let (_, props) = block_by_content(&sut, &driver, title);
        assert_eq!(
            props["option"].as_str(),
            Some(key),
            "option {title:?} must carry `option: {key}`; got {props}"
        );
    }
    let (kept, _) = block_by_content(&sut, &driver, "Kept note");
    assert_eq!(tags_of(&sut, &driver, &kept), vec!["keep"]);

    let file = org_file(&sut, &driver);
    for needle in [
        "? Which store backs the cache? :decision:",
        ":choose: 1",
        ":recommend: a",
        ":option: a",
        ":option: b",
        "* Kept note :keep:",
    ] {
        assert!(
            file.contains(needle),
            "{FILE} must contain {needle:?}; got:\n{file}"
        );
    }
    assert!(
        !file.contains(":keep: :keep:"),
        "a round-tripped tag must not be folded into the title; got:\n{file}"
    );

    // ── Rung 2: update an existing row's tags and drawer. ──
    let (handle, dense) = project(&sut, &driver);
    let edited = dense
        .lines()
        .flat_map(|line| {
            if line.starts_with("* Edited note :old: {#") {
                let alias = &line[line.find("{#").expect("token")..];
                vec![
                    format!("* Edited note :new: {alias}"),
                    ":PROPERTIES:".to_string(),
                    ":owner: agent".to_string(),
                    ":END:".to_string(),
                ]
            } else {
                vec![line.to_string()]
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_ne!(
        edited, dense,
        "the edit must find the `Edited note` row:\n{dense}"
    );
    let applied = patch(&sut, &driver, &handle, &edited);
    assert_eq!(
        (applied["created"].as_u64(), applied["updated"].as_u64()),
        (Some(0), Some(2)),
        "one tag write and one property write; got {applied}"
    );
    sut.settle_projections();
    let (edited_id, props) = block_by_content(&sut, &driver, "Edited note");
    assert_eq!(tags_of(&sut, &driver, &edited_id), vec!["new"]);
    assert_eq!(props["owner"].as_str(), Some("agent"), "got {props}");
    let file = org_file(&sut, &driver);
    assert!(
        file.contains("* Edited note :new:") && file.contains(":owner: agent"),
        "{FILE} must carry the updated tag and property; got:\n{file}"
    );

    // ── Rung 3: deleting the drawer line removes the property. ──
    let (handle, dense) = project(&sut, &driver);
    assert!(
        dense.contains(":owner: agent"),
        "dense_query must render the stored property:\n{dense}"
    );
    let edited = dense
        .lines()
        .filter(|line| !line.starts_with(":owner:"))
        .collect::<Vec<_>>()
        .join("\n");
    let applied = patch(&sut, &driver, &handle, &edited);
    assert_eq!(
        applied["updated"].as_u64(),
        Some(1),
        "one property removal; got {applied}"
    );
    sut.settle_projections();
    let (_, props) = block_by_content(&sut, &driver, "Edited note");
    assert!(
        props.get("owner").is_none(),
        "`owner` must be gone; got {props}"
    );
    let file = org_file(&sut, &driver);
    assert!(
        !file.contains(":owner:"),
        "{FILE} must no longer carry `owner`; got:\n{file}"
    );

    // ── Rung 4: a drawer key naming a storage column refuses the whole patch
    //    before anything is written. ──
    let (handle, dense) = project(&sut, &driver);
    let mut edited = dense.clone();
    if !edited.ends_with('\n') {
        edited.push('\n');
    }
    edited.push_str("* Smuggled row\n:PROPERTIES:\n:parent_id: block:dt-kept\n:END:\n");
    let err = sut
        .runtime()
        .block_on(driver.call_tool_json(
            "dense_patch",
            serde_json::json!({ "handle": handle, "text": edited }),
        ))
        .expect_err("a drawer key naming a storage column must refuse the patch");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("parent_id"),
        "the refusal must name the key; got {msg}"
    );
    assert!(
        rows(
            &sut,
            &driver,
            "SELECT id FROM block_raw WHERE content = 'Smuggled row'"
        )
        .is_empty(),
        "a refused patch must write nothing"
    );

    // ── Rung 5: a title that ends in tag-shaped text, as a verbatim content
    //    write stores it, plans nothing unedited; a retitle reads back exactly
    //    as the agent wrote the row. ──
    sut.runtime()
        .block_on(driver.call_tool_text(
            "execute_operation",
            serde_json::json!({
                "entity_name": "block",
                "operation": "set_field",
                "params": { "id": "block:dt-kept", "field": "content", "value": "Colon note :x:" },
            }),
        ))
        .expect("set_field content over MCP");
    sut.settle_projections();
    let (handle, dense) = project(&sut, &driver);
    let colon_row = |dense: &str| -> String {
        dense
            .lines()
            .find(|line| line.starts_with("* Colon "))
            .unwrap_or_else(|| panic!("no `Colon` row in:\n{dense}"))
            .to_string()
    };
    let before = colon_row(&dense);
    let applied = patch(&sut, &driver, &handle, &dense);
    assert_eq!(
        (applied["created"].as_u64(), applied["updated"].as_u64()),
        (Some(0), Some(0)),
        "an unedited projection writes nothing; got {applied} for {before:?}"
    );
    let (handle, dense) = project(&sut, &driver);
    let renamed = before.replacen("Colon note", "Colon renamed", 1);
    patch(
        &sut,
        &driver,
        &handle,
        &dense.replacen(&before, &renamed, 1),
    );
    sut.settle_projections();
    let (_, dense) = project(&sut, &driver);
    let headline = |line: &str| {
        use holon_org_format::OrgBlockExt;
        let row = holon_org_format::parse_dense(line)
            .expect("a dense row parses")
            .blocks
            .remove(0);
        (row.block.org_title(), row.block.tags())
    };
    assert_eq!(
        headline(&colon_row(&dense)),
        headline(&renamed),
        "the retitled row must read back as written:\n{dense}"
    );

    // ── Rung 6: keys and values org carries only as spelled — a column name in
    //    another casing, an empty value, surrounding space — reach the store
    //    and the file and read back as written. ──
    let (handle, dense) = project(&sut, &driver);
    let rows_text =
        "* Spelled keys\n:PROPERTIES:\n:Content: kept\n:empty:\n:padded: \" padded \"\n:END:";
    let mut edited = dense.clone();
    if !edited.ends_with('\n') {
        edited.push('\n');
    }
    edited.push_str(rows_text);
    patch(&sut, &driver, &handle, &edited);
    sut.settle_projections();
    let (_, props) = block_by_content(&sut, &driver, "Spelled keys");
    assert_eq!(
        (
            props["Content"].as_str(),
            props["empty"].as_str(),
            props["padded"].as_str()
        ),
        (Some("kept"), Some(""), Some(" padded ")),
        "the properties must reach block_raw as written; got {props}"
    );
    let file = org_file(&sut, &driver);
    for needle in [":Content: kept", ":empty:", ":padded: \" padded \""] {
        assert!(
            file.contains(needle),
            "{FILE} must contain {needle:?}; got:\n{file}"
        );
    }
    let (_, dense) = project(&sut, &driver);
    let shown = &dense[dense.find("* Spelled keys").expect("the row is projected")..];
    for line in rows_text.lines().skip(1) {
        assert!(
            shown.contains(line),
            "dense_query must show {line:?} again; got:\n{dense}"
        );
    }

    // ── Rung 7: drawer line order is written — a reordered existing drawer
    //    and a new row's drawer read back, in the store's projection and in
    //    the file, in the order the agent wrote them. ──
    let (handle, dense) = project(&sut, &driver);
    let shown = ":Content: kept\n:empty: \n:padded: \" padded \"";
    let reordered = ":padded: \" padded \"\n:Content: kept\n:empty:";
    assert!(
        dense.contains(shown),
        "the drawer is shown in key order:\n{dense}"
    );
    let new_row = "* Ordered new\n:PROPERTIES:\n:zeta: 1\n:alpha: 2\n:END:";
    let mut edited = dense.replacen(shown, reordered, 1);
    edited.push_str(new_row);
    patch(&sut, &driver, &handle, &edited);
    sut.settle_projections();
    let (_, dense) = project(&sut, &driver);
    let file = org_file(&sut, &driver);
    for needle in [reordered, ":zeta: 1\n:alpha: 2"] {
        assert!(
            dense.contains(needle),
            "dense_query must show the drawer as written, {needle:?}; got:\n{dense}"
        );
        assert!(
            file.contains(needle),
            "{FILE} must hold the drawer as written, {needle:?}; got:\n{file}"
        );
    }
}
