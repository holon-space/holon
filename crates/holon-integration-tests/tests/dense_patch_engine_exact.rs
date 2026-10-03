//! **A `dense_patch` edit reads back exactly from the real store.**
//!
//! The oracle is the contract itself: after the real tool applies an edited
//! projection, `dense_query` over the same rows parses to the same rows as the
//! edited text — title, state, tags, drawer lines in order, body and tree.
//! Every row here has what a hand-written store model got wrong: a multi-line
//! body, a task state, a title the engine's keyword convergence would rewrite.
//!
//! Drives the real tool over the wire against the composed `full_headless`
//! session, then reads the store and the org file on disk.
//!
//! Requires `--features pbt`.
//!
//! The row-text table and the generated property seed one page per case
//! through the keystone's `WriteOrgFile` and draw their edits from the
//! keystone's dense generators (`holon_integration_tests::pbt::dense_text`).
//!
//! @pbt kind harness
//! @pbt covers dense-patch-engine-exact — a retitle keeps the body, a body
//!   edit and a new row's body are written, a state removal and a
//!   keyword-headed title read back as written, a projection whose body reads
//!   as a row refuses every patch, and every part of a row's text and every
//!   generated edit reads back exactly or is refused by name with the store
//!   and the file unchanged

use std::cell::RefCell;
use std::collections::BTreeMap;

use holon_api::EntityUri;
use holon_api::Tags;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::types::TaskState;
use holon_integration_tests::McpUserDriver;
use holon_integration_tests::pbt::composed::embedded_mcp::EmbeddedMcp;
use holon_integration_tests::pbt::composed::embedded_mcp::connect_embedded_mcp;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::CONVERGE_BUDGET;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref_for;
use holon_integration_tests::pbt::dense_text::DenseRows;
use holon_integration_tests::pbt::dense_text::NewRowPlace;
use holon_integration_tests::pbt::dense_text::NewRowText;
use holon_integration_tests::pbt::dense_text::RowTextEdit;
use holon_integration_tests::pbt::dense_text::body_line_shape;
use holon_integration_tests::pbt::dense_text::dense_body;
use holon_integration_tests::pbt::dense_text::dense_properties;
use holon_integration_tests::pbt::dense_text::dense_state;
use holon_integration_tests::pbt::dense_text::dense_tags;
use holon_integration_tests::pbt::dense_text::dense_title;
use holon_integration_tests::pbt::dense_text::new_row_place;
use holon_integration_tests::pbt::dense_text::new_row_place_shape;
use holon_integration_tests::pbt::dense_text::new_row_text;
use holon_integration_tests::pbt::dense_text::row_text_edit;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::transitions::WriteOrgFile;
use holon_org_format::OrgBlockExt;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

type Sut = ComposedSut<WideE2E>;

const FILE: &str = "dense_exact.org";
const QUERY: &str = "SELECT * FROM block WHERE parent_id = 'block:ex-home' ORDER BY sort_key";

fn heading(id: &str, parent: &str, content: &str, state: Option<&str>) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), EntityUri::block(parent), content);
    b.set_property("ID", Value::String(id.to_string()));
    b.set_task_state(state.map(TaskState::from_keyword));
    b
}

fn page_file() -> WriteOrgFile {
    let mut body = heading(
        "ex-body",
        "ex-home",
        "Head\nbody line\nsecond body line",
        None,
    );
    body.set_property("owner", Value::String("me".to_string()));
    WriteOrgFile {
        filename: FILE.to_string(),
        blocks: vec![
            heading("ex-home", "gen-placeholder", "Exact home", None),
            body,
            heading("ex-plain", "ex-home", "Plan", None),
            heading("ex-task", "ex-home", "Task", Some("TODO")),
            heading("ex-combo", "ex-home", "Combo\nold body", Some("TODO")),
        ],
        keyword_set: None,
    }
}

fn boot() -> (Sut, EmbeddedMcp) {
    holon_integration_tests::pbt::set_loro_peer_id_if_unset("1");
    let ref_state = wide_e2e_ref();
    let sut = Sut::init_test(&ref_state);
    let t = E2ETransition::WriteOrgFile(page_file());
    let mut ref_state: ReferenceState = ref_state;
    assert!(WideE2EMachine::preconditions(&ref_state, &t));
    ref_state = WideE2EMachine::apply(ref_state, &t);
    let sut = <Sut as StateMachineTest>::apply(sut, &ref_state, t);
    sut.settle_projections();
    let driver = connect_embedded_mcp(&sut, "dense-patch-engine-exact");
    (sut, driver)
}

fn query(sut: &Sut, driver: &McpUserDriver) -> serde_json::Value {
    sut.runtime()
        .block_on(driver.call_tool_json(
            "dense_query",
            serde_json::json!({ "query": QUERY, "language": "holon_sql" }),
        ))
        .expect("dense_query over MCP")
}

fn project(sut: &Sut, driver: &McpUserDriver) -> (String, String) {
    let proj = query(sut, driver);
    (
        proj["projection_handle"]
            .as_str()
            .unwrap_or_else(|| panic!("no handle: {proj}"))
            .to_string(),
        proj["dense_org"]
            .as_str()
            .unwrap_or_else(|| panic!("no dense_org: {proj}"))
            .to_string(),
    )
}

fn patch(sut: &Sut, driver: &McpUserDriver, handle: &str, text: &str) -> anyhow::Result<()> {
    sut.runtime()
        .block_on(driver.call_tool_json(
            "dense_patch",
            serde_json::json!({ "handle": handle, "text": text }),
        ))
        .map(|_| ())
}

fn stored_content(sut: &Sut, driver: &McpUserDriver, id: &str) -> String {
    let result = sut
        .runtime()
        .block_on(driver.execute_raw_sql(&format!(
            "SELECT content FROM block_raw WHERE id = 'block:{id}'"
        )))
        .expect("read block_raw");
    result["rows"][0]["content"]
        .as_str()
        .unwrap_or_else(|| panic!("block {id} has no content: {result}"))
        .to_string()
}

fn org_file(sut: &Sut, driver: &McpUserDriver) -> String {
    org_file_named(sut, driver, FILE)
}

/// One row as org reads it, with its parent as an index into the same list.
#[derive(Debug, PartialEq, Eq)]
struct Row {
    parent: Option<usize>,
    /// The headline's stars.
    level: i64,
    title: String,
    state: Option<String>,
    tags: Vec<String>,
    drawer: Vec<(String, String)>,
    body: Option<String>,
}

fn rows(text: &str) -> Vec<Row> {
    let parsed = holon_org_format::parse_dense(text)
        .unwrap_or_else(|e| panic!("dense text parses: {e:#}\n{text}"));
    let ids: Vec<&EntityUri> = parsed.blocks.iter().map(|r| &r.parse_id).collect();
    parsed
        .blocks
        .iter()
        .map(|r| Row {
            parent: r
                .parent_parse_id
                .as_ref()
                .map(|p| ids.iter().position(|id| *id == p).expect("parent row")),
            level: r.block.level(),
            title: r.block.org_title(),
            state: r.block.task_state().map(|s| s.keyword),
            tags: r.block.tags().iter().cloned().collect(),
            drawer: r.drawer.clone(),
            body: r.block.body(),
        })
        .collect()
}

/// The contract: the store, queried again, shows the edited rows.
fn assert_reads_back(sut: &Sut, driver: &McpUserDriver, edited: &str, what: &str) {
    sut.settle_projections();
    let (_, now) = project(sut, driver);
    assert_eq!(
        rows(&now),
        rows(edited),
        "{what}: the store does not read back as the edited text\nedited:\n{edited}\nre-queried:\n{now}"
    );
}

fn replace(dense: &str, from: &str, to: &str) -> String {
    assert!(dense.contains(from), "{from:?} is not in:\n{dense}");
    dense.replacen(from, to, 1)
}

/// Apply `edit` to a fresh projection and require it to read back exactly.
fn round_trip(sut: &Sut, driver: &McpUserDriver, what: &str, edit: impl Fn(&str) -> String) {
    let (handle, dense) = project(sut, driver);
    let edited = edit(&dense);
    patch(sut, driver, &handle, &edited)
        .unwrap_or_else(|e| panic!("{what}: the patch must apply: {e:#}\n{edited}"));
    assert_reads_back(sut, driver, &edited, what);
}

#[test]
fn a_retitle_keeps_the_body() {
    let (sut, driver) = boot();
    let (_, dense) = project(&sut, &driver);
    assert!(
        dense.contains("body line\nsecond body line"),
        "dense_query shows the body lines:\n{dense}"
    );
    round_trip(&sut, &driver, "retitle of a row with a body", |d| {
        replace(d, "* Head {#", "* New head {#")
    });
    assert_eq!(
        stored_content(&sut, &driver, "ex-body"),
        "New head\nbody line\nsecond body line"
    );
}

#[test]
fn a_body_edit_is_written() {
    let (sut, driver) = boot();
    round_trip(&sut, &driver, "body edit", |d| {
        replace(d, "second body line", "edited body line")
    });
}

#[test]
fn a_bare_link_is_written_bare() {
    let (sut, driver) = boot();
    round_trip(&sut, &driver, "a bare link", |dense| {
        replace(
            dense,
            "second body line",
            "see [[https://example.com]] here",
        )
    });
    let file = org_file(&sut, &driver);
    assert!(
        file.contains("see [[https://example.com]] here"),
        "org keeps a bare link bare:\n{file}"
    );
}

#[test]
fn a_new_rows_body_is_written() {
    let (sut, driver) = boot();
    round_trip(&sut, &driver, "new row with a body", |d| {
        format!("{}\n* Fresh row\nfresh body\n", d.trim_end())
    });
}

#[test]
fn a_state_removal_reads_back_and_reaches_the_file() {
    let (sut, driver) = boot();
    round_trip(&sut, &driver, "state removal", |d| {
        replace(d, "* TODO Task {#", "* Task {#")
    });
    let file = org_file(&sut, &driver);
    assert!(
        file.contains("** Task\n") && !file.contains("TODO Task"),
        "{FILE} must hold the row without its keyword:\n{file}"
    );
}

#[test]
fn a_keyword_headed_title_on_a_newly_tasked_row_is_not_converged() {
    let (sut, driver) = boot();
    round_trip(&sut, &driver, "keyword-headed title", |d| {
        replace(d, "* Plan {#", "* TODO TODO Plan {#")
    });
}

#[test]
fn a_retitle_body_edit_and_state_change_in_one_patch() {
    let (sut, driver) = boot();
    round_trip(&sut, &driver, "retitle + body + state", |d| {
        let d = replace(d, "* TODO Combo {#", "* DONE Combo renamed {#");
        replace(&d, "old body", "new body\nsecond new line")
    });
}

/// A body line starting with `*` is block text: dense_query shows it with one
/// more leading comma (D230.a), an unchanged projection plans nothing, and a
/// retitle of the row keeps the line.
#[test]
fn a_body_line_starting_with_a_star_is_text() {
    let (sut, driver) = boot();
    sut.runtime()
        .block_on(driver.call_tool_text(
            "execute_operation",
            serde_json::json!({
                "entity_name": "block",
                "operation": "set_field",
                "params": { "id": "block:ex-plain", "field": "content", "value": "Star\n* Bar" },
            }),
        ))
        .expect("set_field content over MCP");
    sut.settle_projections();
    let proj = query(&sut, &driver);
    assert_eq!(proj["unfaithful_rows"], serde_json::json!({}), "{proj}");
    let dense = proj["dense_org"].as_str().expect("dense_org");
    assert!(
        dense.contains("\n,* Bar\n"),
        "the line is shown escaped:\n{dense}"
    );
    patch(
        &sut,
        &driver,
        proj["projection_handle"].as_str().expect("handle"),
        dense,
    )
    .unwrap_or_else(|e| panic!("an unchanged projection plans nothing: {e:#}"));
    round_trip(
        &sut,
        &driver,
        "a retitle of a row with a `*` body line",
        |d| replace(d, "* Star {#", "* Starred {#"),
    );
    assert_eq!(stored_content(&sut, &driver, "ex-plain"), "Starred\n* Bar");
}

/// A row's new text holds at most `MAX_EMPHASIS_MARKS_PER_ROW` emphasis marks:
/// a line at the bound is written exactly, one mark more is refused by the
/// row's name with the bound, and the store keeps its text.
#[test]
fn a_row_over_the_emphasis_mark_bound_is_refused_by_name() {
    let bound = holon_mcp::dense_patch::MAX_EMPHASIS_MARKS_PER_ROW;
    let marks = |n: usize| vec!["foo_bar"; n].join(" ");
    let (sut, driver) = boot();
    let (handle, dense) = project(&sut, &driver);
    let headline = dense
        .lines()
        .find(|l| l.starts_with("* Head {#"))
        .unwrap_or_else(|| panic!("no Head row:\n{dense}"));
    let alias = &headline[headline.find("{#").expect("token")..];
    let over = replace(&dense, "second body line", &marks(bound + 1));
    let msg = format!(
        "{:#}",
        patch(&sut, &driver, &handle, &over).expect_err("a row over the bound is refused")
    );
    for needle in [format!("row {alias}"), bound.to_string()] {
        assert!(
            msg.contains(&needle),
            "the refusal must name {needle:?}: {msg}"
        );
    }
    assert_eq!(
        stored_content(&sut, &driver, "ex-body"),
        "Head\nbody line\nsecond body line"
    );
    round_trip(&sut, &driver, "a row at the emphasis mark bound", |d| {
        replace(d, "second body line", &marks(bound))
    });
    assert_eq!(
        stored_content(&sut, &driver, "ex-body"),
        format!("Head\nbody line\n{}", marks(bound))
    );
}

// ---------------------------------------------------------------------------
// Many edits, one session: every case seeds its own page through the
// keystone's `WriteOrgFile`, edits it, and judges the result against the
// store.
// ---------------------------------------------------------------------------

/// Far above a patch's ~1-2 s, far below the binary's 10 min cap.
const PATCH_ANSWERS_WITHIN: std::time::Duration = std::time::Duration::from_secs(60);

/// The settle budget for seeding a page of `blocks` blocks. Ingest costs
/// ~20 ms per block in the test profile under load (2001 blocks settle in
/// 30-47 s).
fn seed_settle_budget(blocks: usize) -> std::time::Duration {
    const PER_BLOCK: std::time::Duration = std::time::Duration::from_millis(40);
    CONVERGE_BUDGET.max(PER_BLOCK * blocks as u32)
}

/// A booted session that seeds one page per case.
struct Session {
    ref_state: ReferenceState,
    sut: Sut,
    driver: EmbeddedMcp,
    pages: usize,
}

#[derive(Clone, Debug)]
struct SeedRow {
    title: String,
    body: Vec<String>,
    state: Option<String>,
    tags: Tags,
    properties: BTreeMap<String, String>,
    /// Rows nested one level under this one.
    children: Vec<SeedRow>,
}

impl SeedRow {
    fn plain(title: &str) -> SeedRow {
        SeedRow {
            title: title.to_string(),
            body: Vec::new(),
            state: None,
            tags: Tags::default(),
            properties: BTreeMap::new(),
            children: Vec::new(),
        }
    }

    fn block(&self, id: &str, parent: &str) -> Block {
        let content = std::iter::once(self.title.clone())
            .chain(self.body.iter().cloned())
            .collect::<Vec<_>>()
            .join("\n");
        let mut b = heading(id, parent, &content, self.state.as_deref());
        b.tags = self.tags.clone();
        for (k, v) in &self.properties {
            b.set_property(k, Value::String(v.clone()));
        }
        b
    }
}

/// A seeded page: its home block and its rows' ids.
struct Page {
    home: String,
    file: String,
    rows: Vec<String>,
    /// The blocks the file was written with.
    blocks: Vec<Block>,
}

impl Page {
    fn query(&self) -> String {
        let parents: Vec<String> = std::iter::once(&self.home)
            .chain(&self.rows)
            .map(|id| format!("'block:{id}'"))
            .collect();
        format!(
            "SELECT * FROM block WHERE parent_id IN ({}) ORDER BY sort_key",
            parents.join(",")
        )
    }
}

impl Session {
    fn boot() -> Session {
        Session::boot_from(wide_e2e_ref())
    }

    /// A session whose block writes go to the SQL authority, which offers no
    /// batch rollback.
    fn boot_sql_only() -> Session {
        use holon_pbt_core::StorageAdapter;
        Session::boot_from(wide_e2e_ref_for(&holon_pbt_core::Wiring::custom(
            vec![StorageAdapter::Org, StorageAdapter::Turso],
            vec![],
            vec![],
        )))
    }

    fn boot_from(ref_state: ReferenceState) -> Session {
        holon_integration_tests::pbt::set_loro_peer_id_if_unset("1");
        let sut = Sut::init_test(&ref_state);
        let driver = connect_embedded_mcp(&sut, "dense-patch-engine-exact");
        Session {
            ref_state,
            sut,
            driver,
            pages: 0,
        }
    }

    fn seed(self, rows: &[SeedRow]) -> (Session, Page) {
        self.seed_with(rows, None)
    }

    fn seed_with(
        self,
        rows: &[SeedRow],
        keyword_set: Option<holon_integration_tests::pbt::generators::TodoKeywordSet>,
    ) -> (Session, Page) {
        let n = self.pages;
        let home = format!("pp{n}-home");
        let mut ids: Vec<String> = Vec::new();
        let mut blocks = vec![heading(
            &home,
            "gen-placeholder",
            &format!("Page {n}"),
            None,
        )];
        fn push(
            row: &SeedRow,
            id: String,
            parent: &str,
            blocks: &mut Vec<Block>,
            ids: &mut Vec<String>,
        ) {
            blocks.push(row.block(&id, parent));
            for (k, child) in row.children.iter().enumerate() {
                push(child, format!("{id}-c{k}"), &id, blocks, ids);
            }
            ids.push(id);
        }
        for (j, row) in rows.iter().enumerate() {
            push(row, format!("pp{n}-r{j}"), &home, &mut blocks, &mut ids);
        }
        let file = format!("dense_page_{n}.org");
        let mut session = self.write_file(&file, blocks.clone(), keyword_set);
        session.pages += 1;
        (
            session,
            Page {
                home,
                file,
                rows: ids,
                blocks,
            },
        )
    }

    /// `file` written with `blocks` under `keyword_set`, as an editor saves it.
    fn write_file(
        self,
        file: &str,
        blocks: Vec<Block>,
        keyword_set: Option<holon_integration_tests::pbt::generators::TodoKeywordSet>,
    ) -> Session {
        let budget = seed_settle_budget(blocks.len());
        let t = E2ETransition::WriteOrgFile(WriteOrgFile {
            filename: file.to_string(),
            blocks,
            keyword_set,
        });
        let Session {
            ref_state,
            sut,
            driver,
            pages,
        } = self;
        assert!(WideE2EMachine::preconditions(&ref_state, &t));
        let ref_state = WideE2EMachine::apply(ref_state, &t);
        let sut = sut.apply_settling_within(&ref_state, t, budget);
        sut.settle_projections();
        Session {
            ref_state,
            sut,
            driver,
            pages,
        }
    }

    /// The page's whole subtree as the store holds it now — rows a patch
    /// nested under a new row included.
    fn subtree_query(&self, page: &Page) -> String {
        format!(
            "SELECT * FROM block WHERE id IN ({}) ORDER BY sort_key",
            self.subtree_ids(page)
                .iter()
                .map(|id| format!("'{id}'"))
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    /// The ids of the page's whole subtree as the store holds it now.
    fn subtree_ids(&self, page: &Page) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        let mut frontier = vec![format!("block:{}", page.home)];
        while !frontier.is_empty() {
            let result = self
                .sut
                .runtime()
                .block_on(self.driver.execute_raw_sql(&format!(
                    "SELECT id FROM block WHERE parent_id IN ({})",
                    frontier
                        .iter()
                        .map(|id| format!("'{id}'"))
                        .collect::<Vec<_>>()
                        .join(",")
                )))
                .expect("read the page subtree");
            frontier = result["rows"]
                .as_array()
                .expect("rows")
                .iter()
                .map(|r| r["id"].as_str().expect("id").to_string())
                .collect();
            ids.extend(frontier.iter().cloned());
        }
        ids
    }

    /// The page's subtree as its ORG FILE holds it, in dense text: the file
    /// parsed by the org parser and projected like the store.
    fn file_dense(&self, page: &Page) -> String {
        let ids: std::collections::HashSet<String> = self.subtree_ids(page).into_iter().collect();
        let content = org_file_named(&self.sut, &self.driver, &page.file);
        let parsed = holon_org_format::parse_org_file(
            std::path::Path::new(&page.file),
            &content,
            &EntityUri::no_parent(),
            std::path::Path::new(""),
        )
        .unwrap_or_else(|e| panic!("the org file parses: {e:#}\n{content}"));
        let blocks: Vec<Block> = parsed
            .blocks
            .into_iter()
            .filter(|b| ids.contains(b.id.as_str()))
            .collect();
        let vocabulary = holon_org_format::TaskKeywordVocabulary::from_declared(
            holon_org_format::OrgDocumentExt::todo_keywords(&parsed.document),
        );
        holon_mcp::dense_projection::build_projection(
            blocks,
            &holon_mcp::dense_projection::DocVocabularies::Uniform(vocabulary),
        )
        .expect("the file's rows project")
        .dense_text
    }

    /// Every row of the page holds the same task state, keyword and category,
    /// in the store and in its org file.
    fn categories_agree(&self, page: &Page) -> Result<(), String> {
        let ids = self.subtree_ids(page);
        let stored = self
            .sut
            .runtime()
            .block_on(self.driver.execute_raw_sql(&format!(
                "SELECT id, json_extract(properties, '$.task_state') AS keyword, \
                 json_extract(properties, '$.task_state_category') AS category FROM block_raw \
                 WHERE id IN ({})",
                ids.iter()
                    .map(|id| format!("'{id}'"))
                    .collect::<Vec<_>>()
                    .join(",")
            )))
            .expect("read block_raw");
        let content = org_file_named(&self.sut, &self.driver, &page.file);
        let parsed = holon_org_format::parse_org_file(
            std::path::Path::new(&page.file),
            &content,
            &EntityUri::no_parent(),
            std::path::Path::new(""),
        )
        .map_err(|e| format!("the org file parses: {e:#}\n{content}"))?;
        let text = |row: &serde_json::Value, key: &str| {
            row[key]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        for row in stored["rows"].as_array().expect("rows") {
            let id = row["id"].as_str().expect("id");
            let in_store = text(row, "keyword").map(|k| (k, text(row, "category")));
            let in_file = parsed
                .blocks
                .iter()
                .find(|b| b.id.as_str() == id)
                .ok_or_else(|| format!("{id} is not in the org file:\n{content}"))?
                .task_state()
                .map(|s| (s.keyword, Some(s.category.as_str().to_string())));
            if in_store != in_file {
                return Err(format!(
                    "{id}: the store holds the task state {in_store:?}, the org file {in_file:?}"
                ));
            }
        }
        Ok(())
    }

    /// The page's document block holds the title its org file gives the page.
    fn page_title_agrees(&self, page: &Page) -> Result<(), String> {
        let content = org_file_named(&self.sut, &self.driver, &page.file);
        let parsed = holon_org_format::parse_org_file(
            std::path::Path::new(&page.file),
            &content,
            &EntityUri::no_parent(),
            std::path::Path::new(""),
        )
        .map_err(|e| format!("the org file parses: {e:#}\n{content}"))?;
        let stored = self
            .sut
            .runtime()
            .block_on(self.driver.execute_raw_sql(&format!(
                "SELECT content FROM block_raw WHERE id = (SELECT parent_id FROM block_raw WHERE \
                 id = 'block:{}')",
                page.home
            )))
            .expect("read the page's document block");
        let in_store = stored["rows"][0]["content"].as_str().map(str::to_string);
        let in_file = Some(parsed.document.content.clone());
        if in_store != in_file {
            return Err(format!(
                "the store holds the page title {in_store:?}, the org file {in_file:?}\n{content}"
            ));
        }
        Ok(())
    }

    fn query(&self, page: &Page) -> (String, String) {
        let proj = self
            .sut
            .runtime()
            .block_on(self.driver.call_tool_json(
                "dense_query",
                serde_json::json!({ "query": self.subtree_query(page), "language": "holon_sql" }),
            ))
            .expect("dense_query over MCP");
        (
            proj["projection_handle"]
                .as_str()
                .expect("handle")
                .to_string(),
            proj["dense_org"].as_str().expect("dense_org").to_string(),
        )
    }

    /// A call that does not answer within [`PATCH_ANSWERS_WITHIN`] is an error,
    /// so a hung tool fails its case instead of the whole test binary.
    fn patch(&self, handle: &str, text: &str, delete: &[String]) -> anyhow::Result<()> {
        self.sut
            .runtime()
            .block_on(async {
                tokio::time::timeout(
                    PATCH_ANSWERS_WITHIN,
                    self.driver.call_tool_json(
                        "dense_patch",
                        serde_json::json!({ "handle": handle, "text": text, "delete": delete }),
                    ),
                )
                .await
            })
            .map_err(|_| {
                anyhow::anyhow!("dense_patch did not answer within {PATCH_ANSWERS_WITHIN:?}")
            })?
            .map(|_| ())
    }

    /// Everything a refused patch must leave as it was: the page's rows, their
    /// tags, and the org file.
    fn snapshot(&self, page: &Page) -> (serde_json::Value, serde_json::Value, String) {
        let parents: Vec<String> = std::iter::once(&page.home)
            .chain(&page.rows)
            .map(|id| format!("'block:{id}'"))
            .collect();
        let rows = self
            .sut
            .runtime()
            .block_on(self.driver.execute_raw_sql(&format!(
                "SELECT id, parent_id, content, properties FROM block_raw WHERE parent_id IN ({}) \
                 ORDER BY id",
                parents.join(",")
            )))
            .expect("read block_raw");
        let tags = self
            .sut
            .runtime()
            .block_on(self.driver.execute_raw_sql(&format!(
                "SELECT block_id, tag FROM block_tags WHERE block_id LIKE 'block:{}%' ORDER BY \
                 block_id, tag",
                page.home.trim_end_matches("home")
            )))
            .expect("read block_tags");
        (
            rows["rows"].clone(),
            tags["rows"].clone(),
            org_file_named(&self.sut, &self.driver, &page.file),
        )
    }
}

fn org_file_named(sut: &Sut, driver: &McpUserDriver, file: &str) -> String {
    let disk = sut
        .runtime()
        .block_on(driver.call_tool_json("read_org_file", serde_json::json!({ "doc_id": file })))
        .unwrap_or_else(|e| panic!("read_org_file {file}: {e:#}"));
    disk["content"].as_str().expect("content").to_string()
}

/// Each row's words as the text spells them (`holon_org_format::row_lines`):
/// what the agent wrote, before any parser reads it.
fn row_texts(text: &str) -> Vec<Vec<String>> {
    DenseRows::split(text)
        .rows
        .into_iter()
        .map(|row| {
            holon_org_format::row_lines(&row.join("\n"))
                .unwrap_or_else(|e| panic!("a row of the text: {e:#}\n{text}"))
        })
        .collect()
}

/// Whether a refusal names a row the way the agent's text names it: `row {#a}`
/// for a token the text or the delete list holds, `new row "…"` only when the
/// text holds a row without a token, or `the page header` for the text before
/// the first row. A file the agent never wrote names nothing.
fn names_a_row(msg: &str, text: &str, delete: &[String]) -> bool {
    if msg.contains("dense_projection") {
        return false;
    }
    let names_a_token = msg.match_indices("row {#").any(|(at, _)| {
        let alias = msg[at + "row {#".len()..]
            .split('}')
            .next()
            .unwrap_or_default()
            .trim_end_matches('^');
        text.contains(&format!("{{#{alias}}}"))
            || text.contains(&format!("{{#{alias}^}}"))
            || delete.iter().any(|d| d == alias)
    });
    let has_new_row = DenseRows::split(text)
        .rows
        .iter()
        .any(|row| !row[0].contains("{#"));
    names_a_token || (has_new_row && msg.contains("new row \"")) || msg.contains("the page header")
}

/// A character org reads in a headline tag (emacs 30.2 `org-get-tags`: a
/// character of the Unicode categories L*, M*, Nd or Nl, and `_@#%`), plus
/// `-`, which Holon's org dialect reads as a tag character and org does not.
fn org_tag_char(c: char) -> bool {
    use unicode_properties::GeneralCategory;
    use unicode_properties::GeneralCategoryGroup;
    use unicode_properties::UnicodeGeneralCategory;
    matches!(
        c.general_category_group(),
        GeneralCategoryGroup::Letter | GeneralCategoryGroup::Mark
    ) || matches!(
        c.general_category(),
        GeneralCategory::DecimalNumber | GeneralCategory::LetterNumber
    ) || matches!(c, '_' | '@' | '#' | '%' | '-')
}

/// The tags org reads in a dense headline line before the `{#alias}` token:
/// the leftmost match of org 9.7.11's `\(:[[:alnum:]_@#%:]+:\)[ \t]*$`, which
/// needs no blank before the group.
fn org_tags(headline: &str) -> Vec<String> {
    let line = headline.trim_end();
    let line = match line.rfind("{#") {
        Some(at) if line.ends_with('}') => line[..at].trim_end(),
        _ => line,
    };
    let Some(group) = line.char_indices().find_map(|(at, c)| {
        let rest = &line[at..];
        (c == ':'
            && rest.chars().count() >= 3
            && rest.ends_with(':')
            && rest.chars().all(|c| c == ':' || org_tag_char(c)))
        .then_some(rest)
    }) else {
        return Vec::new();
    };
    let mut tags: Vec<String> = group
        .split(':')
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    tags.sort();
    tags.dedup();
    tags
}

/// Drawer keys the store holds as fields, never as properties.
const FIELD_KEYS: &[&str] = &["task_state", "task_state_category", "widget_only"];

/// What emacs 30.2 / org 9.7.11 reads for a whole file, generated by
/// `fixtures/org_keywords/generate.el`.
const ORG_KEYWORDS: &str = include_str!("fixtures/org_keywords/org_keywords.txt");

/// The entries of one `[section]` of [`ORG_KEYWORDS`].
fn org_keywords(section: &str) -> Vec<&'static str> {
    let mut lines = ORG_KEYWORDS.lines().filter(|l| !l.starts_with('#'));
    let header = format!("[{section}]");
    assert!(
        lines.by_ref().any(|l| l == header),
        "no {header} in the org keyword fixture"
    );
    lines.take_while(|l| !l.starts_with('[')).collect()
}

/// Keywords org reads from any line of a file for the whole page.
fn file_keywords() -> Vec<&'static str> {
    org_keywords("file-keywords")
}

/// Drawer keys org computes and never reads from a drawer, in any case.
fn org_special_properties() -> Vec<&'static str> {
    org_keywords("special-properties")
}

/// Holon's file keywords are the keywords org reads for the whole file.
#[test]
fn every_keyword_org_reads_for_the_whole_file_is_a_file_keyword() {
    let mut holon: Vec<&str> = holon_org_format::FileKeyword::ALL
        .iter()
        .map(|k| k.key())
        .collect();
    holon.sort_unstable();
    assert_eq!(holon, file_keywords());
}

/// Why org reads `text` otherwise than any store could hold it, from the
/// text alone: such a text must be refused.
fn org_reads_otherwise(text: &str) -> Option<String> {
    if text.contains('\r') {
        return Some("a carriage return".to_string());
    }
    for row in DenseRows::split(text).rows {
        let mut keys: Vec<&str> = Vec::new();
        let mut in_drawer = false;
        for line in &row[1..] {
            let lower = line.trim().to_ascii_lowercase();
            if lower == ":properties:" {
                in_drawer = true;
            } else if lower == ":end:" {
                in_drawer = false;
            } else if in_drawer {
                if let Some(key) = line
                    .trim()
                    .strip_prefix(':')
                    .and_then(|l| l.split(':').next())
                {
                    keys.push(key);
                }
            } else if file_keywords()
                .iter()
                .any(|k| lower.starts_with(&format!("#+{}:", k.to_ascii_lowercase())))
            {
                return Some(format!("org reads {line:?} for the whole page"));
            }
        }
        for (i, key) in keys.iter().enumerate() {
            if FIELD_KEYS.iter().any(|f| key.eq_ignore_ascii_case(f)) {
                return Some(format!("the drawer key {key:?} names a field"));
            }
            if org_special_properties()
                .iter()
                .any(|f| key.eq_ignore_ascii_case(f))
            {
                return Some(format!("org computes the drawer key {key:?}"));
            }
            if let Some(other) = keys[..i].iter().find(|k| k.eq_ignore_ascii_case(key)) {
                return Some(format!("org reads {other:?} and {key:?} as one property"));
            }
        }
    }
    None
}

/// The store's tags, row by row, are the tags org reads in the edited text.
fn tags_read_as_org_reads(session: &Session, page: &Page, text: &str) -> Result<(), String> {
    let ids = session.subtree_ids(page);
    let rows = session
        .sut
        .runtime()
        .block_on(session.driver.execute_raw_sql(&format!(
            "SELECT block_id, tag FROM block_tags WHERE block_id IN ({})",
            ids.iter()
                .map(|id| format!("'{id}'"))
                .collect::<Vec<_>>()
                .join(",")
        )))
        .expect("read block_tags");
    let mut by_block: BTreeMap<String, Vec<String>> =
        ids.iter().map(|id| (id.clone(), Vec::new())).collect();
    for row in rows["rows"].as_array().expect("rows") {
        by_block
            .get_mut(row["block_id"].as_str().expect("block_id"))
            .expect("a tag of a subtree row")
            .push(row["tag"].as_str().expect("tag").to_string());
    }
    let mut stored: Vec<Vec<String>> = by_block
        .into_values()
        .map(|mut t| {
            t.sort();
            t
        })
        .collect();
    let mut read: Vec<Vec<String>> = DenseRows::split(text)
        .rows
        .iter()
        .map(|row| org_tags(&row[0]))
        .collect();
    stored.sort();
    read.sort();
    if stored != read {
        return Err(format!(
            "the store's tags are not the tags org reads in the text\nstore: {stored:?}\n\
             org: {read:?}\n{text}"
        ));
    }
    Ok(())
}

/// What one edited text did to the store, judged by the contract: it reads
/// back exactly, or it was refused by name and changed nothing. `Err` is a
/// broken contract.
fn judge(
    session: &Session,
    page: &Page,
    handle: &str,
    text: &str,
    delete: &[String],
) -> Result<String, String> {
    let before = session.snapshot(page);
    match session.patch(handle, text, delete) {
        Ok(()) => {
            session.sut.settle_projections();
            let (_, now) = session.query(page);
            if rows(&now) != rows(text) {
                return Err(format!(
                    "the store does not read back as the edited text\nedited:\n{text}\n\
                     re-queried:\n{now}\nedited rows: {:?}\nre-queried rows: {:?}",
                    rows(text),
                    rows(&now)
                ));
            }
            if row_texts(&now) != row_texts(text) {
                return Err(format!(
                    "the store does not show the text the agent wrote\nedited:\n{text}\n\
                     re-queried:\n{now}"
                ));
            }
            let file = session.file_dense(page);
            if row_texts(&file) != row_texts(text) {
                return Err(format!(
                    "the org file does not hold the text the agent wrote\nedited:\n{text}\n\
                     file:\n{file}"
                ));
            }
            session.categories_agree(page)?;
            session.page_title_agrees(page)?;
            if rows(&file) != rows(text) {
                return Err(format!(
                    "the org file does not read back as the edited text\nedited:\n{text}\n\
                     file:\n{file}\nedited rows: {:?}\nfile rows: {:?}",
                    rows(text),
                    rows(&file)
                ));
            }
            Ok("reads back exactly, store and file".to_string())
        }
        Err(e) => {
            let msg = format!("{e:#}");
            if !names_a_row(&msg, text, delete) {
                return Err(format!("the refusal names no row: {msg}\n{text}"));
            }
            if msg.contains("op(s) landed, but the store or the org file does not read back") {
                return Err(format!(
                    "the planner admitted an edit that is not written exactly; its writes \
                     landed and were taken back: {msg}\n{text}"
                ));
            }
            session.sut.settle_projections();
            let after = session.snapshot(page);
            if after != before {
                return Err(format!(
                    "a refused patch changed the store or the file: {msg}\n{text}\nbefore:\n\
                     {before:?}\nafter:\n{after:?}"
                ));
            }
            Ok(format!("refused: {msg}"))
        }
    }
}

/// Every part of a row's text an agent can change: each reads back exactly
/// (an edit, or org renders it as the projection showed it), or is refused by
/// name and changes nothing.
#[test]
fn every_part_of_a_row_reads_back_exactly_or_is_refused() {
    let rows = [
        SeedRow {
            title: "Plan work".to_string(),
            body: vec!["a body line".to_string()],
            state: Some("TODO".to_string()),
            tags: Tags::from_tag_iter(["alpha".to_string(), "beta".to_string()]),
            properties: BTreeMap::from([
                ("owner".to_string(), "me".to_string()),
                ("area".to_string(), "ops".to_string()),
            ]),
            children: Vec::new(),
        },
        SeedRow::plain("Bare row"),
    ];
    type Variant = fn(&str) -> String;
    fn e(dense: &str, from: &str, to: &str) -> String {
        assert!(dense.contains(from), "{from:?} is not in:\n{dense}");
        dense.replacen(from, to, 1)
    }
    let variants: Vec<(&str, Variant)> = vec![
        ("extra spaces after the stars", |d| {
            e(d, "* TODO", "*    TODO")
        }),
        ("trailing space on the headline", |d| {
            e(d, "{#0}\n", "{#0}   \n")
        }),
        ("doubled space in the title", |d| {
            e(d, "Plan work", "Plan  work")
        }),
        ("tag order", |d| e(d, ":alpha:beta:", ":beta:alpha:")),
        ("space before the tag group", |d| {
            e(d, "work :alpha", "work     :alpha")
        }),
        ("space before the token", |d| {
            e(d, ":beta: {#0}", ":beta:    {#0}")
        }),
        ("no space before the token", |d| {
            e(d, ":beta: {#0}", ":beta:{#0}")
        }),
        ("gap marker", |d| e(d, "{#0}", "{#0^}")),
        ("keyword in lower case", |d| e(d, "TODO Plan", "todo Plan")),
        ("keyword changed", |d| e(d, "TODO Plan", "DONE Plan")),
        ("keyword removed", |d| e(d, "TODO Plan", "Plan")),
        ("keyword-headed title", |d| {
            e(d, "TODO Plan", "TODO TODO Plan")
        }),
        ("priority cookie", |d| e(d, "TODO Plan", "TODO [#A] Plan")),
        ("lower-case priority cookie", |d| {
            e(d, "TODO Plan", "TODO [#a] Plan")
        }),
        ("CRLF line ends", |d| d.replace('\n', "\r\n")),
        ("indented drawer line", |d| {
            e(d, ":owner: me", "   :owner: me")
        }),
        ("indented drawer markers", |d| {
            e(d, ":PROPERTIES:\n", "  :PROPERTIES:\n")
        }),
        ("space around the value", |d| {
            e(d, ":owner: me", ":owner:     me   ")
        }),
        ("tab before the value", |d| {
            e(d, ":owner: me", ":owner:\tme")
        }),
        ("drawer markers in lower case", |d| {
            e(d, ":PROPERTIES:", ":properties:")
        }),
        ("end marker in lower case", |d| e(d, ":END:", ":end:")),
        ("key case", |d| e(d, ":owner: me", ":Owner: me")),
        ("value as a literal", |d| {
            e(d, ":owner: me", ":owner: \"me\"")
        }),
        ("drawer lines swapped", |d| {
            if d.contains(":area: ops\n:owner: me") {
                e(d, ":area: ops\n:owner: me", ":owner: me\n:area: ops")
            } else {
                e(d, ":owner: me\n:area: ops", ":area: ops\n:owner: me")
            }
        }),
        ("blank line before the drawer", |d| {
            e(d, "{#0}\n:PROPERTIES:", "{#0}\n\n:PROPERTIES:")
        }),
        ("blank line after the drawer", |d| {
            e(d, ":END:\n", ":END:\n\n")
        }),
        ("blank lines at the end", |d| format!("{d}\n\n\n")),
        ("comment line under the row", |d| {
            e(d, ":END:\n", ":END:\n# a note\n")
        }),
        ("empty drawer on a bare row", |d| {
            e(d, "Bare row {#1}", "Bare row {#1}\n:PROPERTIES:\n:END:")
        }),
        ("drawer without an end", |d| e(d, ":END:\n", "")),
        ("planning line", |d| {
            e(d, "{#0}\n", "{#0}\nSCHEDULED: <2026-09-01 Tue>\n")
        }),
        ("body line edited", |d| {
            e(d, "a body line", "an edited line")
        }),
        ("body line added", |d| {
            e(d, "a body line", "a body line\nand another")
        }),
        ("body removed", |d| e(d, "a body line\n", "")),
        ("retitle keeps the body", |d| {
            e(d, "Plan work", "Plan renamed")
        }),
        ("body on a bare row", |d| {
            e(d, "Bare row {#1}", "Bare row {#1}\nfresh body")
        }),
        ("new row with a body and a state", |d| {
            format!("{}\n* TODO New row\nnew body\n", d.trim_end())
        }),
        ("token inside the title", |d| {
            e(d, "Bare row {#1}", "Bare {#0} row {#1}")
        }),
        ("leading space on the first row", |d| {
            e(d, "* TODO Plan", " * TODO Plan")
        }),
        ("body line of one star", |d| e(d, "a body line", "*")),
        ("body line of two stars", |d| e(d, "a body line", "**")),
        ("body line of five stars", |d| e(d, "a body line", "*****")),
        ("body line of a star and a tab", |d| {
            e(d, "a body line", "*\t")
        }),
        ("body line of a star, a tab and a word", |d| {
            e(d, "a body line", "*\tTab")
        }),
        ("body line of a star and a space", |d| {
            e(d, "a body line", "* ")
        }),
        ("tags after the token", |d| {
            e(d, ":alpha:beta: {#0}", "{#0} :alpha:beta:")
        }),
        ("body line of five equals signs", |d| {
            e(d, "a body line", "x =====")
        }),
        ("body line of a hundred slashes and a word", |d| {
            e(d, "a body line", &format!("{} x", "/".repeat(100)))
        }),
        ("body line of 99 stars", |d| {
            e(d, "a body line", &"*".repeat(99))
        }),
        ("bold word in the body", |d| {
            e(d, "a body line", "a *bold* line")
        }),
        ("new row with a body of five equals signs", |d| {
            format!("{}\n* New row\nx =====\n", d.trim_end())
        }),
        ("first row two levels deep", |d| {
            e(d, "* TODO Plan", "** TODO Plan")
        }),
        ("second row three levels deep", |d| {
            e(d, "* Bare row", "*** Bare row")
        }),
        ("tags after the token on an indented first row", |d| {
            e(
                d,
                "* TODO Plan work :alpha:beta: {#0}",
                " * TODO Plan work {#0} :alpha:",
            )
        }),
    ];
    let mut session = Session::boot();
    let mut broken = Vec::new();
    for (name, variant) in variants {
        let (next, page) = session.seed(&rows);
        session = next;
        let (handle, dense) = session.query(&page);
        match judge(&session, &page, &handle, &variant(&dense), &[]) {
            Ok(outcome)
                if name == "drawer lines swapped" && !outcome.starts_with("reads back exactly") =>
            {
                broken.push(format!(
                    "{name}: a drawer reorder is an edit dense_patch writes: {outcome}"
                ))
            }
            Ok(outcome) => println!("[row text] {name}: {outcome}"),
            Err(e) => broken.push(format!("{name}: {e}")),
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n\n"));
}

/// Text org writes with other bytes, or reads for the whole page, is refused
/// by the row's name before any write; an authored drawer key is written as
/// authored. Each reads back exactly or is refused, never taken back.
#[test]
fn a_text_org_writes_otherwise_is_refused_before_any_write() {
    let authored = |key: &str| holon_org_format::AuthoredKey::new(key).property();
    let rows = [
        SeedRow {
            title: "Plan".to_string(),
            body: vec!["line one".to_string(), "line two".to_string()],
            properties: BTreeMap::from([
                ("owner".to_string(), "me".to_string()),
                (authored("_secret"), "s".to_string()),
                (authored("org_properties"), "op".to_string()),
                (authored("_drawer_order"), "dd".to_string()),
            ]),
            ..SeedRow::plain("Plan")
        },
        SeedRow {
            body: vec!["other body".to_string()],
            ..SeedRow::plain("Other")
        },
    ];
    type Variant = fn(&str) -> String;
    fn e(dense: &str, from: &str, to: &str) -> String {
        assert!(dense.contains(from), "{from:?} is not in:\n{dense}");
        dense.replacen(from, to, 1)
    }
    let refused: Vec<(&str, Variant)> = vec![
        ("#+ line in an example block", |d| {
            e(
                d,
                "line one",
                "#+begin_example\n#+foo bar\n#+end_example\nline one",
            )
        }),
        ("#+ line in an export block", |d| {
            e(
                d,
                "line one",
                "#+begin_export html\n#+foo bar\n#+end_export\nline one",
            )
        }),
        ("#+TODO: line in an example block", |d| {
            e(
                d,
                "line one",
                "#+begin_example\n#+TODO: XX | YY\n#+end_example\nline one",
            )
        }),
        ("babel call", |d| {
            e(d, "line one", "#+CALL: foo()\nline one")
        }),
        ("affiliated keyword and babel call", |d| {
            e(d, "line one", "#+ATTR_LATEX: :x y\n#+CALL: f()\nline one")
        }),
        ("caption with blank lines around it", |d| {
            e(d, "{#1}\nother body", "{#1}\n\n#+CAPTION: a\n\nother body")
        }),
        ("#+FILETAGS: line", |d| {
            e(d, "line one", "#+FILETAGS: :x:\nline one")
        }),
        ("#+CATEGORY: line", |d| {
            e(d, "line one", "#+CATEGORY: c\nline one")
        }),
        ("#+PROPERTY: line", |d| {
            e(d, "line one", "#+PROPERTY: foo bar\nline one")
        }),
        ("#+FILETAGS: line on a new row", |d| {
            format!("{}\n* Fresh\n#+FILETAGS: :x:\n", d.trim_end())
        }),
        ("task_state drawer key", |d| {
            e(d, ":owner: me", ":owner: me\n:Task_State: x")
        }),
        ("task_state_category drawer key", |d| {
            e(d, ":owner: me", ":owner: me\n:TASK_STATE_CATEGORY: done")
        }),
        ("WIDGET_ONLY drawer value org cannot read", |d| {
            e(d, ":owner: me", ":owner: me\n:WIDGET_ONLY: banana")
        }),
        ("WIDGET_ONLY drawer line", |d| {
            e(d, ":owner: me", ":owner: me\n:WIDGET_ONLY: t")
        }),
        ("task_state drawer key on a new row", |d| {
            format!(
                "{}\n* Fresh\n:PROPERTIES:\n:task_state: x\n:END:\n",
                d.trim_end()
            )
        }),
        ("a key beside its case variant", |d| {
            e(d, ":owner: me", ":owner: me\n:Owner: x")
        }),
        ("a key beside its case variant on a new row", |d| {
            format!(
                "{}\n* Fresh\n:PROPERTIES:\n:area: a\n:AREA: b\n:END:\n",
                d.trim_end()
            )
        }),
        ("#+TITLE: line in a verse block", |d| {
            e(
                d,
                "line one",
                "#+begin_verse\n#+TITLE: inverse\n#+end_verse\nline one",
            )
        }),
        ("#+TITLE: line in a comment block", |d| {
            e(
                d,
                "other body",
                "#+begin_comment\n#+TITLE: incomment\n#+end_comment\nother body",
            )
        }),
        ("#+TITLE: line in a verse block on a new row", |d| {
            format!(
                "{}\n* Fresh\n#+begin_verse\n#+TITLE: inverse\n#+end_verse\n",
                d.trim_end()
            )
        }),
        ("tags after the token and a field drawer key", |d| {
            e(
                &e(d, "Plan {#0}", "Plan {#0} :alpha:"),
                ":owner: me",
                ":owner: me\n:Task_State: x",
            )
        }),
        ("tags after the token on an indented first row", |d| {
            e(d, "* Plan {#0}", " * Plan {#0} :alpha:")
        }),
        ("an id keyword before the first row", |d| {
            e(d, "* Plan {#0}", "#+ID: block:x\n* Plan {#0}")
        }),
        ("a source block before the first row", |d| {
            e(
                d,
                "* Plan {#0}",
                "#+begin_src sh :id block:x\necho\n#+end_src\n* Plan {#0}",
            )
        }),
        ("a file drawer before the first row", |d| {
            e(d, "* Plan {#0}", ":PROPERTIES:\n:ID: x\n:END:\n* Plan {#0}")
        }),
        ("ITEM drawer key", |d| {
            e(d, ":owner: me", ":owner: me\n:ITEM: banana")
        }),
        ("CLOSED drawer key", |d| {
            e(d, ":owner: me", ":owner: me\n:CLOSED: banana")
        }),
        ("FILE drawer key", |d| {
            e(d, ":owner: me", ":owner: me\n:FILE: banana")
        }),
        ("BLOCKED drawer key in lower case", |d| {
            e(d, ":owner: me", ":owner: me\n:blocked: banana")
        }),
        ("a link whose label is its target", |d| {
            e(
                d,
                "line one",
                "see [[https://u.example][https://u.example]]",
            )
        }),
        ("an entity link whose label is its target", |d| {
            e(d, "line one", "see [[block:u][block:u]]")
        }),
        ("ALLTAGS drawer key on a new row", |d| {
            format!(
                "{}\n* Fresh\n:PROPERTIES:\n:ALLTAGS: x\n:END:\n",
                d.trim_end()
            )
        }),
    ];
    let exact: Vec<(&str, Variant)> = vec![
        ("caption with blank lines around it, under a drawer", |d| {
            e(d, ":END:\nline one", ":END:\n\n#+CAPTION: a\n\nline one")
        }),
        ("new drawer key that starts with a backslash", |d| {
            e(d, ":owner: me", ":owner: me\n:\\_x: v")
        }),
        ("authored _secret line removed", |d| {
            e(d, ":_secret: s\n", "")
        }),
        ("authored org_properties line removed", |d| {
            e(d, ":org_properties: op\n", "")
        }),
        ("authored _drawer_order line removed", |d| {
            e(d, ":_drawer_order: dd\n", "")
        }),
        (",#+TITLE: line in a verse block", |d| {
            e(
                d,
                "line one",
                "#+begin_verse\n,#+TITLE: inverse\n#+end_verse\nline one",
            )
        }),
    ];
    let keyword_lines: Vec<(String, String)> = file_keywords()
        .into_iter()
        .map(|k| (format!("#+{k}: line"), format!("#+{k}: v")))
        .collect();
    let named = |variants: Vec<(&str, Variant)>| {
        variants
            .into_iter()
            .map(|(name, variant)| {
                (
                    name.to_string(),
                    Box::new(variant) as Box<dyn Fn(&str) -> String>,
                )
            })
            .collect::<Vec<_>>()
    };
    let mut refused = named(refused);
    for (name, line) in keyword_lines {
        refused.push((
            name,
            Box::new(move |d: &str| e(d, "line one", &format!("{line}\nline one"))),
        ));
    }
    let mut session = Session::boot();
    let mut broken = Vec::new();
    for (want, variants) in [("refused", refused), ("reads back exactly", named(exact))] {
        for (name, variant) in variants {
            let (next, page) = session.seed(&rows);
            session = next;
            let (handle, dense) = session.query(&page);
            match judge(&session, &page, &handle, &variant(&dense), &[]) {
                Ok(outcome) if outcome.starts_with(want) => {
                    println!("[org text] {name}: {outcome}")
                }
                Ok(outcome) => broken.push(format!("{name}: wanted {want}, got {outcome}")),
                Err(e) => broken.push(format!("{name}: {e}")),
            }
        }
    }
    assert!(broken.is_empty(), "{}", broken.join("\n\n"));
}

/// A row whose stored body org reads with a raw `#+` line in an example block
/// keeps it through an edit of its title, and the same line re-spelled `,#+`
/// is refused, since the store keeps the authored spelling.
#[test]
fn a_stored_authored_body_survives_an_edit_of_its_title() {
    let body = "#+begin_example\n#+foo bar\n#+end_example";
    let mut row = SeedRow {
        body: body.lines().map(str::to_string).collect(),
        ..SeedRow::plain("Plan")
    };
    row.properties.insert(
        holon_org_format::org_props::AUTHORED_TEXT.to_string(),
        serde_json::Value::String(body.to_string()).to_string(),
    );
    let (session, page) = Session::boot().seed(&[row.clone(), SeedRow::plain("Other")]);
    let (handle, dense) = session.query(&page);
    assert!(
        dense.contains("\n#+foo bar\n"),
        "the row shows its authored line:\n{dense}"
    );
    let outcome = judge(
        &session,
        &page,
        &handle,
        &replace(&dense, "* Plan {#", "* Renamed {#"),
        &[],
    )
    .unwrap_or_else(|broken| panic!("{broken}"));
    assert!(outcome.starts_with("reads back exactly"), "{outcome}");

    let (session, page) = session.seed(&[row, SeedRow::plain("Other")]);
    let (handle, dense) = session.query(&page);
    let outcome = judge(
        &session,
        &page,
        &handle,
        &replace(&dense, "\n#+foo bar\n", "\n,#+foo bar\n"),
        &[],
    )
    .unwrap_or_else(|broken| panic!("{broken}"));
    assert!(outcome.starts_with("refused"), "{outcome}");
}

/// A row whose stored body holds inline marks shows them, and keeps them
/// through an edit of its title and an edit of its body.
#[test]
fn a_stored_marked_body_survives_an_edit_of_its_row() {
    let seed = |session: Session| {
        let n = session.pages;
        let home = format!("pp{n}-home");
        let row = format!("pp{n}-r0");
        let (content, marks) = holon_org_format::extract_inline_marks(
            "Plan\na *bold* and [[https://x.org][link]] line",
        );
        let mut marked = heading(&row, &home, &content, None);
        marked.marks = Some(marks);
        let blocks = vec![
            heading(&home, "gen-placeholder", &format!("Page {n}"), None),
            marked,
            heading(&format!("pp{n}-r1"), &home, "Other", None),
        ];
        let file = format!("dense_page_{n}.org");
        let mut session = session.write_file(&file, blocks.clone(), None);
        session.pages += 1;
        let page = Page {
            home,
            file,
            rows: vec![row, format!("pp{n}-r1")],
            blocks,
        };
        (session, page)
    };
    let (session, page) = seed(Session::boot());
    let (handle, dense) = session.query(&page);
    assert!(
        dense.contains("\na *bold* and [[https://x.org][link]] line\n"),
        "the row shows its marks:\n{dense}"
    );
    let outcome = judge(
        &session,
        &page,
        &handle,
        &replace(&dense, "* Plan {#", "* Renamed {#"),
        &[],
    )
    .unwrap_or_else(|broken| panic!("{broken}"));
    assert!(outcome.starts_with("reads back exactly"), "{outcome}");

    let (session, page) = seed(session);
    let (handle, dense) = session.query(&page);
    let outcome = judge(
        &session,
        &page,
        &handle,
        &replace(&dense, " line\n", " line, edited\n"),
        &[],
    )
    .unwrap_or_else(|broken| panic!("{broken}"));
    assert!(outcome.starts_with("reads back exactly"), "{outcome}");
}

#[derive(Clone, Debug)]
struct Case {
    rows: Vec<SeedRow>,
    edits: Vec<(usize, RowTextEdit)>,
    new_row: Option<NewRowText>,
    new_row_place: NewRowPlace,
    /// Whether the page's file declares its own `#+TODO: NEXT | DONE` ring.
    ring: bool,
    /// A row to delete, and whether its descendant rows leave the text with
    /// it (else they stay, under the row before it).
    delete: Option<(usize, bool)>,
    /// Whether the text ends its lines with CRLF.
    crlf: bool,
}

fn leaf_row() -> impl Strategy<Value = SeedRow> {
    (
        dense_title(),
        dense_body(),
        dense_state(),
        dense_tags(),
        dense_properties(),
    )
        .prop_map(|(title, body, state, tags, properties)| SeedRow {
            title,
            body,
            state,
            tags,
            properties,
            children: Vec::new(),
        })
}

fn seed_row() -> impl Strategy<Value = SeedRow> {
    (leaf_row(), prop::collection::vec(leaf_row(), 0..3)).prop_map(|(mut row, children)| {
        row.children = children;
        row
    })
}

fn case() -> impl Strategy<Value = Case> {
    (
        prop::collection::vec(seed_row(), 1..4),
        prop::collection::vec((0usize..4, row_text_edit()), 1..3),
        prop::option::weighted(0.6, new_row_text()),
        prop::option::weighted(0.5, (0usize..6, any::<bool>())),
        new_row_place(),
        prop::bool::weighted(0.3),
        prop::bool::weighted(0.05),
    )
        .prop_map(
            |(rows, edits, new_row, delete, new_row_place, ring, crlf)| Case {
                rows,
                edits,
                new_row,
                new_row_place,
                ring,
                delete,
                crlf,
            },
        )
}

/// The shapes a case exercises, by name.
fn reach(case: &Case) -> Vec<&'static str> {
    let mut hit = Vec::new();
    for (row, edit) in &case.edits {
        let has_body = !case.rows[row % case.rows.len()].body.is_empty();
        match edit {
            RowTextEdit::Retitle(t) if t.starts_with("TODO ") || t.starts_with("DONE ") => {
                hit.push("keyword-headed retitle")
            }
            RowTextEdit::Retitle(_) if has_body => hit.push("retitle of a row with a body"),
            RowTextEdit::SetBody(lines) => {
                hit.push("body edit");
                hit.extend(lines.iter().map(|line| body_line_shape(line)));
            }
            RowTextEdit::SetState(None) if case.rows[row % case.rows.len()].state.is_some() => {
                hit.push("state removal")
            }
            RowTextEdit::SetState(Some(_)) => hit.push("state set"),
            RowTextEdit::MoveToFront => hit.push("move"),
            RowTextEdit::Demote if row % case.rows.len() == 0 => {
                hit.extend(["nest", "first row demoted"])
            }
            RowTextEdit::Demote => hit.push("nest"),
            RowTextEdit::SetProperty(key, _)
                if FIELD_KEYS.iter().any(|f| key.eq_ignore_ascii_case(f)) =>
            {
                hit.push("typed drawer key")
            }
            RowTextEdit::SetProperty(key, _) if key == "Owner" || key == "AREA" => {
                hit.push("case-variant drawer key")
            }
            RowTextEdit::SetProperty(key, _) if !key.chars().all(|c| c.is_ascii_lowercase()) => {
                hit.push("unusual drawer key")
            }
            RowTextEdit::SetProperty(..) | RowTextEdit::DropProperty(_) => hit.push("drawer edit"),
            RowTextEdit::Retag(tags) if tags.iter().any(|t| !t.chars().all(org_tag_char)) => {
                hit.push("tag org reads as title text")
            }
            RowTextEdit::Retag(tags) if tags.iter().any(|t| t.contains(['@', '#', '%', '-'])) => {
                hit.push("tag with a punctuation character")
            }
            _ => {}
        }
    }
    if let Some(row) = case.new_row.as_ref().filter(|r| !r.body.is_empty()) {
        hit.push("new row with a body");
        hit.extend(row.body.iter().map(|line| body_line_shape(line)));
    }
    fn stored_lines(rows: &[SeedRow], hit: &mut Vec<&'static str>) {
        for row in rows {
            hit.extend(row.body.iter().map(|line| body_line_shape(line)));
            stored_lines(&row.children, hit);
        }
    }
    stored_lines(&case.rows, &mut hit);
    hit.sort_unstable();
    hit.dedup();
    hit
}

/// Generated stores with multi-line bodies, states, tags and drawers, and
/// generated edits of them: each reads back exactly from the real store, or is
/// refused by name and changes nothing.
#[test]
fn a_generated_dense_edit_reads_back_exactly_or_is_refused() {
    generated_dense_edits(Session::boot());
}

/// [`a_generated_dense_edit_reads_back_exactly_or_is_refused`] on the SQL
/// write authority, which places rows by its own sort keys.
#[test]
fn a_generated_dense_edit_on_the_sql_authority_reads_back_exactly_or_is_refused() {
    generated_dense_edits(Session::boot_sql_only());
}

fn generated_dense_edits(session: Session) {
    const CASES: u32 = 96;
    let session = RefCell::new(Some(session));
    let counts: RefCell<BTreeMap<&'static str, usize>> = RefCell::new(BTreeMap::new());
    // A seeded draw, so the reach floor below is a fixed property of the
    // generator and cannot flake.
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: CASES,
            max_shrink_iters: 24,
            failure_persistence: None,
            ..Config::default()
        },
        proptest::test_runner::TestRng::deterministic_rng(
            proptest::test_runner::RngAlgorithm::ChaCha,
        ),
    );
    let result = runner.run(&case(), |case| {
        let s = session.borrow_mut().take().expect("session");
        let (s, page) = if case.ring {
            // The ring's own keywords stand in for the defaults the rows were
            // drawn with; the edits still draw TODO/DOING, which it lacks.
            let rows: Vec<SeedRow> = case.rows.iter().map(in_next_shipped_ring).collect();
            s.seed_with(&rows, next_shipped_ring())
        } else {
            s.seed(&case.rows)
        };
        let (handle, dense) = s.query(&page);
        let mut text = DenseRows::split(&dense);
        let mut delete = Vec::new();
        let mut delete_shape = None;
        if let Some((row, with_subtree)) = case.delete {
            let depth = |r: &[String]| r[0].len() - r[0].trim_start_matches('*').len();
            let has_children = |i: usize| {
                text.rows
                    .get(i + 1)
                    .is_some_and(|next| depth(next) > depth(&text.rows[i]))
            };
            // Two draws in three prefer a row that has children.
            let prefer_parent = row % 3 != 0;
            let row = (0..text.rows.len())
                .map(|k| (row + k) % text.rows.len())
                .find(|&i| !prefer_parent || has_children(i))
                .unwrap_or(row % text.rows.len());
            let own = depth(&text.rows[row]);
            let subtree = text.rows[row + 1..]
                .iter()
                .take_while(|r| depth(r) > own)
                .count();
            if text.rows.len() > 1 + subtree {
                delete.push(text.alias(row).expect("an existing row carries a token"));
                let gone = if with_subtree { subtree + 1 } else { 1 };
                text.rows.drain(row..row + gone);
                delete_shape = Some(match (subtree > 0, with_subtree) {
                    (false, _) => "delete of a leaf row",
                    (true, true) => "delete of a non-leaf row with its subtree",
                    (true, false) => "delete of a non-leaf row whose children stay",
                });
            }
        }
        // After the delete: the text's nesting still mirrors the store's, so
        // the rows a subtree delete takes out of the text are the store's.
        for (row, edit) in &case.edits {
            let n = text.rows.len();
            text.edit(row % n, edit);
        }
        let new_row_shape = case.new_row.as_ref().map(|new_row| {
            let shape = new_row_place_shape(case.new_row_place, text.rows.len());
            text.insert(new_row, case.new_row_place);
            shape
        });
        let mut edited = text.join();
        if case.crlf {
            edited = edited.replace('\n', "\r\n");
        }
        let judged =
            judge(&s, &page, &handle, &edited, &delete).and_then(
                |outcome| match org_reads_otherwise(&edited) {
                    Some(why) if !outcome.starts_with("refused") => Err(format!(
                        "{why}, so the edit must be refused, but it {outcome}\n{edited}"
                    )),
                    None if !outcome.starts_with("refused") => {
                        tags_read_as_org_reads(&s, &page, &edited).map(|()| outcome)
                    }
                    _ => Ok(outcome),
                },
            );
        *session.borrow_mut() = Some(s);
        let outcome = judged.map_err(TestCaseError::fail)?;
        println!("[case] {outcome}");
        let mut counts = counts.borrow_mut();
        if let Some(shape) = delete_shape {
            *counts.entry(shape).or_default() += 1;
        }
        if let Some(shape) = new_row_shape {
            *counts.entry(shape).or_default() += 1;
        }
        if case.ring {
            *counts.entry("page with its own keyword ring").or_default() += 1;
        }
        if case.crlf {
            *counts.entry("CRLF line ends").or_default() += 1;
        }
        for shape in reach(&case) {
            *counts.entry(shape).or_default() += 1;
        }
        *counts
            .entry(if outcome.starts_with("refused") {
                "refused"
            } else {
                "applied"
            })
            .or_default() += 1;
        Ok(())
    });
    let counts = counts.into_inner();
    println!("[reach] over {CASES} cases: {counts:?}");
    if let Err(e) = result {
        panic!("{e}");
    }
    for shape in [
        "retitle of a row with a body",
        "keyword-headed retitle",
        "body edit",
        "state removal",
        "move",
        "nest",
        "drawer edit",
        "new row with a body",
        "delete of a leaf row",
        "delete of a non-leaf row with its subtree",
        "delete of a non-leaf row whose children stay",
        "new row last",
        "new row before every other row",
        "new row between two rows",
        "new row as a row's first child",
        "page with its own keyword ring",
        "body keyword line",
        "body page keyword line",
        "body escaped keyword line",
        "body table line",
        "body block delimiter",
        "body drawer-shaped line",
        "body rule",
        "body mark run",
        "first row demoted",
        "body footnote",
        "body blank line",
        "body #+ line inside an example or export block",
        "body babel call line",
        "body comma-escaped star line",
        "body verse block",
        "body src block",
        "body center block",
        "body FILETAGS line",
        "body CATEGORY line",
        "body PROPERTY line",
        "typed drawer key",
        "case-variant drawer key",
        "unusual drawer key",
        "tag org reads as title text",
        "tag with a punctuation character",
        "CRLF line ends",
        "applied",
        "refused",
    ] {
        assert!(
            counts.get(shape).copied().unwrap_or(0) >= 2,
            "{shape:?} was drawn fewer than 2 times: {counts:?}"
        );
    }
}

/// The rings a generated document declares. `CLOSING` and `TRIAGE` share
/// their keywords and classify `CANCELLED` differently; `None` is org's
/// defaults.
const RINGS: [Option<&[(&str, bool)]>; 4] = [
    None,
    Some(&[("TODO", false), ("CANCELLED", true)]),
    Some(&[("TODO", false), ("CANCELLED", false), ("DONE", true)]),
    Some(&[("NEXT", false), ("SHIPPED", true)]),
];
const CLOSING: usize = 1;
const TRIAGE: usize = 2;

fn ring_set(ring: usize) -> Option<holon_integration_tests::pbt::generators::TodoKeywordSet> {
    RINGS[ring].map(|words| {
        holon_integration_tests::pbt::generators::TodoKeywordSet(
            words
                .iter()
                .map(|&(word, done)| {
                    if done {
                        TaskState::done(word)
                    } else {
                        TaskState::active(word)
                    }
                })
                .collect(),
        )
    })
}

fn ring_words(ring: usize) -> Vec<&'static str> {
    match RINGS[ring] {
        Some(words) => words.iter().map(|&(word, _)| word).collect(),
        None => vec!["TODO", "DOING", "DONE", "CANCELLED"],
    }
}

#[derive(Clone, Debug)]
struct CrossCase {
    ring_a: usize,
    ring_b: usize,
    /// Per row of page A, the index of its keyword in A's ring (`None`: no
    /// state).
    a_states: Vec<Option<usize>>,
    b_rows: usize,
    moved: usize,
    /// Under B's home (`None`) or under that row of B.
    target: Option<usize>,
    /// A keyword the edit gives the moved row, from B's ring or A's.
    new_state: Option<(bool, usize)>,
    /// Page B's file is saved again with the other of CLOSING/TRIAGE before
    /// the query, as an editor would.
    ring_edit: bool,
    /// The moved row carries one child row, with this keyword index into A's
    /// ring (`Some(None)`: a child with no state).
    child: Option<Option<usize>>,
    /// The move is one `move_block` operation instead of a dense edit.
    move_op: bool,
    /// The child (`true`) or the moved row (`false`), stateless, has a title
    /// headed by a keyword only B's ring declares.
    title_keyword: Option<bool>,
    /// The text also ends with a new top-level row, which no page of the
    /// projection can hold.
    new_top_row: bool,
}

fn cross_case() -> impl Strategy<Value = CrossCase> {
    // CLOSING and TRIAGE are drawn most, so a row moves between the two
    // readings of CANCELLED often.
    let ring =
        || prop_oneof![1 => Just(0usize), 3 => Just(CLOSING), 3 => Just(TRIAGE), 1 => Just(3usize)];
    (
        ring(),
        ring(),
        prop::collection::vec(
            prop::option::weighted(0.8, prop_oneof![1 => 0usize..4, 2 => Just(1usize)]),
            1..3,
        ),
        1usize..3,
        0usize..3,
        prop::option::of(0usize..3),
        prop::option::weighted(0.45, (prop::bool::weighted(0.3), 0usize..4)),
        prop::bool::weighted(0.3),
        prop::option::weighted(0.4, prop::option::weighted(0.8, 0usize..4)),
        prop::bool::weighted(0.35),
        prop::option::weighted(0.35, any::<bool>()),
        prop::bool::weighted(0.2),
    )
        .prop_map(
            |(
                ring_a,
                ring_b,
                a_states,
                b_rows,
                moved,
                target,
                new_state,
                ring_edit,
                child,
                move_op,
                title_keyword,
                new_top_row,
            )| {
                let mut a_states = a_states;
                let (child, new_state, move_op) = match title_keyword {
                    Some(true) => (Some(None), new_state, false),
                    Some(false) => {
                        let row = moved % a_states.len();
                        a_states[row] = None;
                        (child, None, false)
                    }
                    None => (child, new_state, move_op),
                };
                CrossCase {
                    ring_a,
                    ring_b,
                    a_states,
                    b_rows,
                    moved,
                    target,
                    new_state: new_state.filter(|_| !move_op),
                    ring_edit,
                    child,
                    move_op,
                    title_keyword,
                    new_top_row: new_top_row && !move_op,
                }
            },
        )
}

/// Generated cross-document edits: rows of two files with different rings
/// in ONE projection, a row or a row with its child moved from one file to
/// the other by a dense edit (a state set on it or not) or by one
/// `move_block`, and a ring the file saved again with before the query. Every
/// case reads back exactly, in the store and the file, keyword and category,
/// or is refused by name with both files unchanged; a move that hands the
/// target file a keyword its ring lacks is refused.
#[test]
fn a_generated_cross_document_edit_reads_back_exactly_or_is_refused() {
    const CASES: u32 = 32;
    let session = RefCell::new(Some(Session::boot()));
    let counts: RefCell<BTreeMap<&'static str, usize>> = RefCell::new(BTreeMap::new());
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: CASES,
            max_shrink_iters: 16,
            failure_persistence: None,
            ..Config::default()
        },
        proptest::test_runner::TestRng::deterministic_rng(
            proptest::test_runner::RngAlgorithm::ChaCha,
        ),
    );
    let result = runner.run(&cross_case(), |case| {
        let mut hit: Vec<&'static str> = Vec::new();
        let s = session.borrow_mut().take().expect("session");
        let words_a = ring_words(case.ring_a);
        let moved = case.moved % case.a_states.len();
        let child_state = case
            .child
            .map(|state| state.map(|k| words_a[k % words_a.len()].to_string()));
        let ring_b_final = match case.ring_b {
            CLOSING if case.ring_edit => TRIAGE,
            TRIAGE if case.ring_edit => CLOSING,
            ring => ring,
        };
        let title_keyword: Option<(bool, &str)> = case.title_keyword.and_then(|on_child| {
            ring_words(ring_b_final)
                .into_iter()
                .find(|word| !words_a.contains(word))
                .map(|word| (on_child, word))
        });
        let headed = |on_child: bool, title: String| match title_keyword {
            Some((on, word)) if on == on_child => format!("{word} {title}"),
            _ => title,
        };
        let a_rows: Vec<SeedRow> = case
            .a_states
            .iter()
            .enumerate()
            .map(|(i, state)| SeedRow {
                state: state.map(|k| words_a[k % words_a.len()].to_string()),
                children: match &child_state {
                    Some(state) if i == moved => vec![SeedRow {
                        state: state.clone(),
                        ..SeedRow::plain(&headed(true, format!("Alpha{i}Child")))
                    }],
                    _ => Vec::new(),
                },
                ..SeedRow::plain(&if i == moved {
                    headed(false, format!("Alpha{i}"))
                } else {
                    format!("Alpha{i}")
                })
            })
            .collect();
        let b_rows: Vec<SeedRow> = (0..case.b_rows)
            .map(|i| SeedRow::plain(&format!("Beta{i}")))
            .collect();
        let (s, a) = s.seed_with(&a_rows, ring_set(case.ring_a));
        let (mut s, b) = s.seed_with(&b_rows, ring_set(case.ring_b));
        let mut ring_b = case.ring_b;
        if case.ring_edit && [CLOSING, TRIAGE].contains(&ring_b) {
            ring_b = if ring_b == CLOSING { TRIAGE } else { CLOSING };
            s = s.write_file(&b.file, b.blocks.clone(), ring_set(ring_b));
            hit.push("ring edited by saving the file");
        }
        // The session goes back to the cell whatever the verdict, so a
        // shrink step finds it.
        let verdict = (|| -> Result<(), TestCaseError> {
            for page in [&a, &b] {
                s.categories_agree(page)
                    .map_err(|why| TestCaseError::fail(format!("before the patch: {why}")))?;
            }

            let ids: Vec<String> = [&a, &b]
                .iter()
                .flat_map(|page| {
                    std::iter::once(format!("block:{}", page.home)).chain(s.subtree_ids(page))
                })
                .collect();
            let query = format!(
                "SELECT * FROM block WHERE id IN ({}) ORDER BY sort_key",
                ids.iter()
                    .map(|id| format!("'{id}'"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let proj = s
                .sut
                .runtime()
                .block_on(s.driver.call_tool_json(
                    "dense_query",
                    serde_json::json!({ "query": query, "language": "holon_sql" }),
                ))
                .expect("dense_query");
            let handle = proj["projection_handle"].as_str().expect("handle");
            let dense = proj["dense_org"].as_str().expect("dense_org").to_string();

            let mut text = DenseRows::split(&dense);
            let position = |text: &DenseRows, title: &str| {
                text.rows
                    .iter()
                    .position(|r| r[0].contains(&format!(" {title} {{#")))
                    .unwrap_or_else(|| panic!("no {title} row:\n{dense}"))
            };
            let moved_title = format!("Alpha{moved}");
            let from = position(&text, &moved_title);
            let depth = |row: &Vec<String>| row[0].len() - row[0].trim_start_matches('*').len();
            let own = depth(&text.rows[from]);
            let group = 1 + text.rows[from + 1..]
                .iter()
                .take_while(|r| depth(r) > own)
                .count();
            let mut moved_rows: Vec<Vec<String>> = text.rows.drain(from..from + group).collect();
            let mut row = moved_rows.remove(0);
            let (anchor, level) = match case.target {
                None => (
                    format!(
                        "Page {}",
                        b.home.trim_start_matches("pp").trim_end_matches("-home")
                    ),
                    "**",
                ),
                Some(j) => (format!("Beta{}", j % b_rows.len()), "***"),
            };
            let at = position(&text, &anchor);
            let stars = row[0].len() - row[0].trim_start_matches('*').len();
            row[0] = format!("{level}{}", &row[0][stars..]);
            for child in &mut moved_rows {
                let child_stars = depth(child);
                child[0] = format!(
                    "{level}{}{}",
                    "*".repeat(child_stars - stars),
                    &child[0][child_stars..]
                );
            }
            let old_state = a_rows[moved].state.clone();
            let mut final_state = old_state.clone();
            if let Some((from_b, k)) = case.new_state {
                let words = ring_words(if from_b { ring_b } else { case.ring_a });
                let word = words[k % words.len()];
                let head = row[0].trim_start_matches('*').trim_start();
                let title = match &old_state {
                    Some(old) => head
                        .strip_prefix(old.as_str())
                        .expect("keyword")
                        .trim_start(),
                    None => head,
                };
                row[0] = format!("{level} {word} {title}");
                final_state = Some(word.to_string());
                hit.push("state set on the moved row");
                if !ring_words(ring_b).contains(&word) {
                    hit.push("keyword the target ring lacks");
                }
            }
            let lacking: Vec<&str> = final_state
                .iter()
                .chain(child_state.iter().flatten())
                .map(String::as_str)
                .filter(|kw| !ring_words(ring_b).contains(kw))
                .collect();
            if child_state.is_some() {
                hit.push("move of a row with its child");
            }
            if !lacking.is_empty() && case.new_state.is_none() {
                hit.push("keyword the target ring lacks, no state change");
            }
            if child_state
                .iter()
                .flatten()
                .any(|kw| !ring_words(ring_b).contains(&kw.as_str()))
            {
                hit.push("child keyword the target ring lacks");
            }
            let target_block = match case.target {
                None => b.home.clone(),
                Some(j) => b.rows[j % b_rows.len()].clone(),
            };
            text.rows.insert(at + 1, row);
            for (k, child) in moved_rows.into_iter().enumerate() {
                text.rows.insert(at + 2 + k, child);
            }
            if case.new_top_row {
                hit.push("new top-level row in a two-page projection");
                text.rows.push(vec!["* NewTop".to_string()]);
            }
            let edited = text.join();
            let kept = final_state.as_deref().is_some_and(|kw| {
                RINGS[case.ring_a]
                    .zip(RINGS[ring_b])
                    .is_some_and(|(ra, rb)| {
                        match (ra.iter().find(|w| w.0 == kw), rb.iter().find(|w| w.0 == kw)) {
                            (Some(a), Some(b)) => a.1 != b.1,
                            _ => false,
                        }
                    })
            });
            if kept {
                hit.push("cross-ring move of a keyword the rings classify differently");
            }
            if RINGS[ring_b].is_none() {
                hit.push("move into a file on org's defaults");
            }

            let before: Vec<_> = [&a, &b].iter().map(|p| s.snapshot(p)).collect();
            let written = if case.move_op {
                hit.push("move_block operation");
                s.sut
                    .runtime()
                    .block_on(s.driver.call_tool_text(
                        "execute_operation",
                        serde_json::json!({
                            "entity_name": "block",
                            "operation": "move_block",
                            "params": {
                                "id": format!("block:{}", a.home.replace("-home", &format!("-r{moved}"))),
                                "parent_id": format!("block:{target_block}"),
                            },
                        }),
                    ))
                    .map(|_| ())
            } else {
                s.patch(handle, &edited, &[])
            };
            let outcome = match written {
                Ok(()) => {
                    s.sut.settle_projections();
                    let proj = s
                        .sut
                        .runtime()
                        .block_on(s.driver.call_tool_json(
                            "dense_query",
                            serde_json::json!({ "query": query, "language": "holon_sql" }),
                        ))
                        .expect("dense_query");
                    let now = proj["dense_org"].as_str().expect("dense_org");
                    if !case.move_op && row_texts(now) != row_texts(&edited) {
                        return Err(TestCaseError::fail(format!(
                            "the store does not show the text the agent wrote\nedited:\n{edited}\n\
                             re-queried:\n{now}"
                        )));
                    }
                    for page in [&a, &b] {
                        s.categories_agree(page).map_err(TestCaseError::fail)?;
                    }
                    "applied"
                }
                Err(e) => {
                    let msg = format!("{e:#}");
                    if msg.contains("ROLLED BACK")
                        && !msg.contains("the 0 op(s) dispatched before it")
                    {
                        return Err(TestCaseError::fail(format!(
                            "the edit was written and taken back, not refused before the \
                             write: {msg}\n{edited}"
                        )));
                    }
                    let named = if case.move_op {
                        msg.contains("block:pp") && msg.contains("is not a keyword of the document")
                    } else {
                        names_a_row(&msg, &edited, &[])
                    };
                    if !named {
                        return Err(TestCaseError::fail(format!(
                            "the refusal names no row: {msg}\n{edited}"
                        )));
                    }
                    s.sut.settle_projections();
                    let after: Vec<_> = [&a, &b].iter().map(|p| s.snapshot(p)).collect();
                    if after != before {
                        return Err(TestCaseError::fail(format!(
                            "a refused patch changed a file or the store: {msg}\n{edited}"
                        )));
                    }
                    "refused"
                }
            };
            if case.new_top_row && outcome != "refused" {
                return Err(TestCaseError::fail(format!(
                    "a new top-level row has no page in a two-page projection, so the edit must \
                     be refused\n{edited}"
                )));
            }
            if kept
                && lacking.is_empty()
                && title_keyword.is_none()
                && !case.new_top_row
                && outcome != "applied"
            {
                return Err(TestCaseError::fail(format!(
                    "a move between two rings that both declare its keyword must apply\n{edited}"
                )));
            }
            if let Some((on_child, word)) = title_keyword {
                hit.push("title headed by a keyword only the target ring declares");
                if outcome != "refused" {
                    return Err(TestCaseError::fail(format!(
                        "the {} title headed by `{word}` reads as a state in the target file, \
                         so the edit must be refused\n{edited}",
                        if on_child { "child's" } else { "moved row's" }
                    )));
                }
            }
            if !lacking.is_empty() && outcome != "refused" {
                return Err(TestCaseError::fail(format!(
                    "a move that hands {lacking:?} to a file whose ring lacks it must be refused \
                     (move_block: {})\n{edited}",
                    case.move_op
                )));
            }
            hit.push(outcome);
            Ok(())
        })();
        *session.borrow_mut() = Some(s);
        verdict?;
        let mut counts = counts.borrow_mut();
        for shape in hit {
            *counts.entry(shape).or_default() += 1;
        }
        Ok(())
    });
    let counts = counts.into_inner();
    println!("[reach] cross-document over {CASES} cases: {counts:?}");
    if let Err(e) = result {
        panic!("{e}");
    }
    for shape in [
        "cross-ring move of a keyword the rings classify differently",
        "ring edited by saving the file",
        "state set on the moved row",
        "keyword the target ring lacks",
        "keyword the target ring lacks, no state change",
        "move of a row with its child",
        "child keyword the target ring lacks",
        "move_block operation",
        "move into a file on org's defaults",
        "title headed by a keyword only the target ring declares",
        "new top-level row in a two-page projection",
        "applied",
        "refused",
    ] {
        assert!(
            counts.get(shape).copied().unwrap_or(0) >= 2,
            "{shape:?} was drawn fewer than 2 times: {counts:?}"
        );
    }
}

/// The child of a row moved to another file is written to that file: a title
/// headed by a keyword only the target ring declares reads back there as a
/// state, so the patch is refused by the child's name before any write.
#[test]
fn a_moved_rows_child_is_checked_in_the_file_it_moves_to() {
    let (s, a) = Session::boot().seed_with(
        &[SeedRow {
            children: vec![SeedRow::plain("NEXT step")],
            ..SeedRow::plain("Alpha0")
        }],
        ring_set(CLOSING),
    );
    let (s, b) = s.seed_with(&[SeedRow::plain("Beta0")], ring_set(3));
    s.sut.settle_projections();
    let ids: Vec<String> = [&a, &b]
        .iter()
        .flat_map(|page| {
            std::iter::once(format!("'block:{}'", page.home))
                .chain(s.subtree_ids(page).into_iter().map(|id| format!("'{id}'")))
        })
        .collect();
    let query = format!(
        "SELECT * FROM block WHERE id IN ({}) ORDER BY sort_key",
        ids.join(",")
    );
    let proj = s
        .sut
        .runtime()
        .block_on(s.driver.call_tool_json(
            "dense_query",
            serde_json::json!({ "query": query, "language": "holon_sql" }),
        ))
        .expect("dense_query");
    let handle = proj["projection_handle"].as_str().expect("handle");
    let dense = proj["dense_org"].as_str().expect("dense_org").to_string();
    let mut text = DenseRows::split(&dense);
    let at = |text: &DenseRows, title: &str| {
        text.rows
            .iter()
            .position(|r| r[0].contains(title))
            .unwrap_or_else(|| panic!("no {title} row:\n{dense}"))
    };
    let from = at(&text, " Alpha0 {#");
    let moved: Vec<Vec<String>> = text.rows.drain(from..from + 2).collect();
    let to = at(&text, " Beta0 {#");
    for (k, row) in moved.into_iter().enumerate() {
        text.rows.insert(to + 1 + k, row);
    }
    let edited = text.join();
    let before: Vec<_> = [&a, &b].iter().map(|p| s.snapshot(p)).collect();

    let err = s
        .patch(handle, &edited, &[])
        .expect_err("the child would read back in the target file as a NEXT task");

    let msg = format!("{err:#}");
    assert!(
        msg.contains("NEXT step") && !msg.contains("ROLLED BACK"),
        "{msg}\n{edited}"
    );
    s.sut.settle_projections();
    let after: Vec<_> = [&a, &b].iter().map(|p| s.snapshot(p)).collect();
    assert_eq!(after, before, "a refused patch changes nothing: {msg}");
}

/// A new top-level row in a projection of two pages has no page to go to: the
/// refusal names the row and nothing is written.
#[test]
fn zz_r7v_new_top_level_row_in_a_two_page_projection_names_its_row() {
    let mut failures = Vec::new();
    for (leg, boot) in [
        ("loro", Session::boot as fn() -> Session),
        ("sql", Session::boot_sql_only),
    ] {
        let (s, a) = boot().seed(&[SeedRow::plain("Alpha0")]);
        let (s, b) = s.seed(&[SeedRow::plain("Beta0")]);
        s.sut.settle_projections();
        let ids: Vec<String> = [&a, &b]
            .iter()
            .flat_map(|page| {
                std::iter::once(format!("'block:{}'", page.home))
                    .chain(s.subtree_ids(page).into_iter().map(|id| format!("'{id}'")))
            })
            .collect();
        let query = format!(
            "SELECT * FROM block WHERE id IN ({}) ORDER BY sort_key",
            ids.join(",")
        );
        let proj = s
            .sut
            .runtime()
            .block_on(s.driver.call_tool_json(
                "dense_query",
                serde_json::json!({ "query": query, "language": "holon_sql" }),
            ))
            .expect("dense_query");
        let handle = proj["projection_handle"].as_str().expect("handle");
        let dense = proj["dense_org"].as_str().expect("dense_org").to_string();
        let mut text = DenseRows::split(&dense);
        text.rows.push(vec!["* Gamma".to_string()]);
        let edited = text.join();
        let before: Vec<_> = [&a, &b].iter().map(|p| s.snapshot(p)).collect();
        match s.patch(handle, &edited, &[]) {
            Ok(()) => failures.push(format!(
                "[{leg}] the row has no page, so it must be refused"
            )),
            Err(e) => {
                let msg = format!("{e:#}");
                if !names_a_row(&msg, &edited, &[]) {
                    failures.push(format!("[{leg}] the refusal names no row: {msg}"));
                }
                s.sut.settle_projections();
                let after: Vec<_> = [&a, &b].iter().map(|p| s.snapshot(p)).collect();
                if after != before {
                    failures.push(format!(
                        "[{leg}] a refused patch changed a file or the store"
                    ));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// A merge parks the merged-away block's body as its canonical's FIRST child,
/// ahead of the children the canonical already had, on both write authorities.
#[test]
fn a_merged_body_is_parked_as_the_canonicals_first_child() {
    let mut failures = Vec::new();
    for (leg, boot) in [
        ("loro", Session::boot as fn() -> Session),
        ("sql", Session::boot_sql_only),
    ] {
        let (session, page) = boot().seed(&[
            SeedRow {
                children: vec![SeedRow::plain("Kid")],
                ..SeedRow::plain("Canon")
            },
            SeedRow::plain("Dup"),
        ]);
        let merged = session
            .sut
            .runtime()
            .block_on(session.driver.call_tool_text(
                "execute_operation",
                serde_json::json!({
                    "entity_name": "block",
                    "operation": "merge_blocks",
                    "params": {
                        "canonical": "block:pp0-r0",
                        "duplicate": "block:pp0-r1",
                    },
                }),
            ));
        if let Err(e) = merged {
            failures.push(format!("[{leg}] merge_blocks: {e:#}"));
            continue;
        }
        session.sut.settle_projections();
        let (_, now) = session.query(&page);
        let titles: Vec<String> = rows(&now).into_iter().map(|r| r.title).collect();
        if titles != ["Canon", "Dup", "Kid"] {
            failures.push(format!(
                "[{leg}] the parked body is not first: {titles:?}\n{now}"
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn ids_under(session: &Session, page: &Page) -> Vec<String> {
    let parents: Vec<String> = std::iter::once(&page.home)
        .chain(&page.rows)
        .map(|id| format!("'block:{id}'"))
        .collect();
    let rows = session
        .sut
        .runtime()
        .block_on(session.driver.execute_raw_sql(&format!(
            "SELECT id FROM block_raw WHERE parent_id IN ({}) ORDER BY id",
            parents.join(",")
        )))
        .expect("read block_raw");
    rows["rows"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|r| r["id"].as_str().expect("id").to_string())
        .collect()
}

/// `delete: [alias]` removes the row with its subtree, as the tool promises.
#[test]
fn a_deleted_row_takes_its_subtree() {
    let (session, page) = Session::boot().seed(&[
        SeedRow {
            children: vec![SeedRow::plain("Child")],
            ..SeedRow::plain("Parent")
        },
        SeedRow::plain("Stays"),
    ]);
    session.sut.settle_projections();
    let (handle, dense) = session.query(&page);
    let text = DenseRows::split(&dense);
    let parent = text
        .rows
        .iter()
        .position(|r| r[0].contains("Parent"))
        .expect("the parent row");
    let alias = text.alias(parent).expect("token");
    let mut edited = text.clone();
    edited.rows.drain(parent..parent + 2);
    session
        .patch(&handle, &edited.join(), &[alias])
        .unwrap_or_else(|e| panic!("deleting a row with a child must apply: {e:#}\n{dense}"));
    session.sut.settle_projections();
    let (_, now) = session.query(&page);
    assert_eq!(rows(&now), rows(&edited.join()), "re-queried:\n{now}");
    assert_eq!(
        ids_under(&session, &page),
        vec!["block:pp0-r1".to_string()],
        "the parent and its child are gone, the sibling stays"
    );
}

/// A row the text keeps, and does not move, that sits inside a deleted row's
/// subtree cannot stay: the patch is refused by that row's name before any
/// write.
#[test]
fn a_kept_row_inside_a_deleted_subtree_is_refused_by_name_before_any_write() {
    let (session, page) = Session::boot().seed(&[
        SeedRow {
            children: vec![SeedRow {
                children: vec![SeedRow::plain("Leaf")],
                ..SeedRow::plain("Middle")
            }],
            ..SeedRow::plain("Top")
        },
        SeedRow::plain("Other"),
    ]);
    session.sut.settle_projections();
    // The middle row is not selected, so the leaf renders at the top level
    // (gap-marked) and stays there with no move.
    let query = "SELECT * FROM block WHERE id IN ('block:pp0-r0', 'block:pp0-r0-c0-c0', \
                 'block:pp0-r1') ORDER BY sort_key";
    let proj = session
        .sut
        .runtime()
        .block_on(session.driver.call_tool_json(
            "dense_query",
            serde_json::json!({ "query": query, "language": "holon_sql" }),
        ))
        .expect("dense_query");
    let dense = proj["dense_org"].as_str().expect("dense_org").to_string();
    let handle = proj["projection_handle"]
        .as_str()
        .expect("handle")
        .to_string();
    let mut text = DenseRows::split(&dense);
    let top = text
        .rows
        .iter()
        .position(|r| r[0].contains("Top"))
        .expect("top row");
    let leaf = text
        .rows
        .iter()
        .position(|r| r[0].contains("Leaf"))
        .expect("leaf row");
    let leaf_alias = text.alias(leaf).expect("token");
    let alias = text.alias(top).expect("token");
    text.rows.remove(top);
    let before = session.snapshot(&page);
    let err = session
        .patch(&handle, &text.join(), &[alias])
        .expect_err("a kept row inside the deleted subtree must refuse the patch");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(&format!("{{#{leaf_alias}")) && !msg.contains("ROLLED BACK"),
        "the refusal names the kept row and comes before any write: {msg}\n{dense}"
    );
    assert_eq!(
        session.snapshot(&page),
        before,
        "nothing may be written: {msg}"
    );
}

/// dense_query shows the block exactly as stored, or names the row. A stored
/// `TODO Plan` with no state, in a file whose ring lacks `TODO`, is shown as
/// that title under the file's own ring.
#[test]
fn a_stored_keyword_title_the_file_does_not_read_as_a_task_is_shown_as_stored() {
    use holon_integration_tests::pbt::generators::TodoKeywordSet;
    let ring = || {
        Some(TodoKeywordSet(vec![
            TaskState::active("NEXT"),
            TaskState::done("DONE"),
        ]))
    };
    let (session, page) = Session::boot().seed_with(
        &[SeedRow {
            body: vec!["a body line".to_string()],
            ..SeedRow::plain("TODO Plan")
        }],
        ring(),
    );
    let proj = session
        .sut
        .runtime()
        .block_on(session.driver.call_tool_json(
            "dense_query",
            serde_json::json!({ "query": page.query(), "language": "holon_sql" }),
        ))
        .expect("dense_query");
    assert_eq!(proj["unfaithful_rows"], serde_json::json!({}), "{proj}");
    let shown = rows(proj["dense_org"].as_str().expect("dense_org"));
    assert!(
        shown
            .iter()
            .any(|r| r.title == "TODO Plan" && r.state.is_none()),
        "the row shows its stored title and no state: {proj}"
    );

    let (session, page) = session.seed_with(
        &[SeedRow {
            state: Some("NEXT".to_string()),
            ..SeedRow::plain("Plan")
        }],
        ring(),
    );
    let proj = session
        .sut
        .runtime()
        .block_on(session.driver.call_tool_json(
            "dense_query",
            serde_json::json!({ "query": page.query(), "language": "holon_sql" }),
        ))
        .expect("dense_query");
    assert_eq!(
        proj["unfaithful_rows"],
        serde_json::json!({}),
        "a keyword the file declares is shown as stored: {proj}"
    );
}

/// A new row the text puts before every existing row is created first, on
/// both write authorities.
#[test]
fn a_dotted_property_key_reads_back_as_one_key() {
    let mut failures = Vec::new();
    for (leg, boot) in [
        ("loro", Session::boot as fn() -> Session),
        ("sql", Session::boot_sql_only),
    ] {
        let (session, page) = boot().seed(&[SeedRow::plain("One")]);
        let (handle, dense) = session.query(&page);
        let mut text = DenseRows::split(&dense);
        text.rows[0].splice(
            1..1,
            [":PROPERTIES:", ":x.y: 65", ":END:"].map(str::to_string),
        );
        let edited = text.join();
        if let Err(e) = session.patch(&handle, &edited, &[]) {
            failures.push(format!("[{leg}] a dotted key must apply: {e:#}\n{edited}"));
            continue;
        }
        session.sut.settle_projections();
        let (_, now) = session.query(&page);
        if rows(&now) != rows(&edited) {
            failures.push(format!("[{leg}] re-queried differs from the edit:\n{now}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn a_new_row_at_the_front_reads_back_first() {
    let mut failures = Vec::new();
    for (leg, boot) in [
        ("loro", Session::boot as fn() -> Session),
        ("sql", Session::boot_sql_only),
    ] {
        let (session, page) = boot().seed(&[SeedRow::plain("One"), SeedRow::plain("Two")]);
        let (handle, dense) = session.query(&page);
        let mut text = DenseRows::split(&dense);
        text.rows
            .insert(0, vec!["* Front".to_string(), "front body".to_string()]);
        let edited = text.join();
        if let Err(e) = session.patch(&handle, &edited, &[]) {
            failures.push(format!(
                "[{leg}] a new first row must apply: {e:#}\n{edited}"
            ));
            continue;
        }
        session.sut.settle_projections();
        let (_, now) = session.query(&page);
        if rows(&now) != rows(&edited) {
            failures.push(format!("[{leg}] re-queried differs from the edit:\n{now}"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Clearing the state of a row whose stored title starts with a keyword, and
/// retitling it in the same patch, reads back.
#[test]
fn a_state_removal_with_a_retitle_of_a_keyword_headed_title_reads_back() {
    let (session, page) = Session::boot().seed(&[SeedRow {
        state: Some("TODO".to_string()),
        ..SeedRow::plain("TODO Plan")
    }]);
    let (handle, dense) = session.query(&page);
    let edited = replace(&dense, "* TODO TODO Plan {#", "* Renamed {#");
    session
        .patch(&handle, &edited, &[])
        .unwrap_or_else(|e| panic!("the edit must apply: {e:#}\n{edited}"));
    session.sut.settle_projections();
    let (_, now) = session.query(&page);
    assert_eq!(rows(&now), rows(&edited), "re-queried:\n{now}");
}

fn next_done_ring() -> Option<holon_integration_tests::pbt::generators::TodoKeywordSet> {
    Some(holon_integration_tests::pbt::generators::TodoKeywordSet(
        vec![TaskState::active("NEXT"), TaskState::done("DONE")],
    ))
}

/// A ring whose done keyword is not one of org's defaults.
fn next_shipped_ring() -> Option<holon_integration_tests::pbt::generators::TodoKeywordSet> {
    Some(holon_integration_tests::pbt::generators::TodoKeywordSet(
        vec![TaskState::active("NEXT"), TaskState::done("SHIPPED")],
    ))
}

/// `row` with its keywords replaced by the `NEXT | SHIPPED` ring's.
fn in_next_shipped_ring(row: &SeedRow) -> SeedRow {
    let mut row = row.clone();
    row.state = row
        .state
        .as_deref()
        .map(|s| if s == "DONE" { "SHIPPED" } else { "NEXT" }.to_string());
    row.children = row.children.iter().map(in_next_shipped_ring).collect();
    row
}

/// A keyword the row's own file does not declare is title text under that
/// file's ring, which the header shows: it reads back as a title, in the store
/// and the file.
#[test]
fn a_keyword_the_rows_file_does_not_declare_is_title_text() {
    let (session, page) = Session::boot().seed_with(&[SeedRow::plain("Plan")], next_done_ring());
    let (handle, dense) = session.query(&page);
    assert!(
        dense.contains("#+TODO: NEXT | DONE\n"),
        "the header is the file's ring:\n{dense}"
    );
    let edited = replace(&dense, "* Plan {#", "* TODO Plan {#");
    let outcome =
        judge(&session, &page, &handle, &edited, &[]).unwrap_or_else(|broken| panic!("{broken}"));
    assert!(outcome.starts_with("reads back exactly"), "{outcome}");
    assert!(
        rows(&session.file_dense(&page))
            .iter()
            .any(|r| r.title == "TODO Plan" && r.state.is_none()),
        "the file holds the title text"
    );
}

/// A keyword the row's own file declares, and no row uses yet, applies to the
/// store and the file.
#[test]
fn a_keyword_the_rows_file_declares_applies_to_store_and_file() {
    let (session, page) = Session::boot().seed_with(&[SeedRow::plain("Plan")], next_done_ring());
    let (handle, dense) = session.query(&page);
    let edited = replace(&dense, "* Plan {#", "* NEXT Plan {#");
    let outcome =
        judge(&session, &page, &handle, &edited, &[]).unwrap_or_else(|broken| panic!("{broken}"));
    assert!(outcome.starts_with("reads back exactly"), "{outcome}");
}

/// Rows from two files with different rings in ONE projection: each row's
/// keyword is judged by its own file.
#[test]
fn rows_from_two_files_are_judged_by_their_own_rings() {
    let (session, ring_page) =
        Session::boot().seed_with(&[SeedRow::plain("Ringed")], next_done_ring());
    let (session, plain_page) = session.seed(&[SeedRow::plain("Plain")]);
    let query =
        "SELECT * FROM block WHERE id IN ('block:pp0-r0', 'block:pp1-r0') ORDER BY sort_key";
    let project = |session: &Session| {
        let proj = session
            .sut
            .runtime()
            .block_on(session.driver.call_tool_json(
                "dense_query",
                serde_json::json!({ "query": query, "language": "holon_sql" }),
            ))
            .expect("dense_query");
        (
            proj["projection_handle"]
                .as_str()
                .expect("handle")
                .to_string(),
            proj["dense_org"].as_str().expect("dense_org").to_string(),
        )
    };
    let (handle, dense) = project(&session);
    let before = session.snapshot(&ring_page);
    let err = session
        .patch(
            &handle,
            &replace(&dense, "* Ringed {#", "* TODO Ringed {#"),
            &[],
        )
        .expect_err("TODO is not a keyword of the ringed file");
    let msg = format!("{err:#}");
    assert!(msg.contains("{#") && msg.contains("NEXT"), "{msg}\n{dense}");
    session.sut.settle_projections();
    assert_eq!(
        session.snapshot(&ring_page),
        before,
        "nothing may be written: {msg}"
    );

    let (handle, dense) = project(&session);
    let edited = replace(&dense, "* Plain {#", "* TODO Plain {#");
    session
        .patch(&handle, &edited, &[])
        .unwrap_or_else(|e| panic!("TODO is a keyword of the plain file: {e:#}\n{edited}"));
    session.sut.settle_projections();
    assert!(
        rows(&session.file_dense(&plain_page))
            .iter()
            .any(|r| r.title == "Plain" && r.state.as_deref() == Some("TODO")),
        "the plain file holds the keyword"
    );
}

/// A CR in a row's text is refused by the row's name, and nothing is written.
#[test]
fn a_carriage_return_in_a_body_is_refused_by_name() {
    let (session, page) = Session::boot().seed(&[SeedRow {
        body: vec!["a body line".to_string()],
        ..SeedRow::plain("Alpha")
    }]);
    let (handle, dense) = session.query(&page);
    let edited = replace(&dense, "\na body line", "\nbody line\r\nsecond body line");
    let outcome =
        judge(&session, &page, &handle, &edited, &[]).unwrap_or_else(|broken| panic!("{broken}"));
    assert!(
        outcome.starts_with("refused") && outcome.contains("carriage return"),
        "{outcome}"
    );
}

/// A row the agent left as dense_query showed it is no edit, even when org
/// reads its text otherwise: an edit of another row applies.
#[test]
fn an_unchanged_unfaithful_row_does_not_block_an_edit_of_another_row() {
    let (session, page) = Session::boot().seed(&[
        SeedRow::plain("Qeyimy f"),
        SeedRow {
            body: vec![
                "#+jfm: biyx".to_string(),
                "uN R9r9 ey5y0dJ".to_string(),
                "* a]]".to_string(),
            ],
            ..SeedRow::plain("[[https://example.com/vgr][e 28 MXtX af1")
        },
    ]);
    let (handle, dense) = session.query(&page);
    let edited = replace(&dense, "* Qeyimy f {#", "* Renamed {#");
    let outcome =
        judge(&session, &page, &handle, &edited, &[]).unwrap_or_else(|broken| panic!("{broken}"));
    assert!(
        outcome.starts_with("reads back exactly"),
        "{outcome}\n{dense}"
    );
}

/// `* ?` with no text is refused by its row's name before any write, on both
/// authorities: no store or org file holds an empty question.
#[test]
fn an_empty_question_is_refused_by_its_row_before_any_write() {
    for session in [Session::boot(), Session::boot_sql_only()] {
        let (session, page) = session.seed(&[SeedRow {
            state: Some("TODO".to_string()),
            ..SeedRow::plain("Plan")
        }]);
        let (handle, dense) = session.query(&page);
        let rows = DenseRows::split(&dense);
        let alias = rows
            .alias(
                rows.rows
                    .iter()
                    .position(|r| r[0].contains("Plan"))
                    .expect("the Plan row"),
            )
            .expect("token");
        let state = || {
            let rows = session
                .sut
                .runtime()
                .block_on(session.driver.execute_raw_sql(&format!(
                    "SELECT content, properties FROM block_raw WHERE id = 'block:{}'",
                    page.rows[0]
                )))
                .expect("read block_raw");
            let file =
                session
                    .sut
                    .runtime()
                    .block_on(session.driver.call_tool_json(
                        "read_org_file",
                        serde_json::json!({ "doc_id": page.file }),
                    ))
                    .expect("read_org_file");
            (rows["rows"].clone(), file["content"].clone())
        };
        let before = state();
        let edited = replace(&dense, "* TODO Plan {#", "* ? {#");
        let err = session
            .patch(&handle, &edited, &[])
            .expect_err("an empty question is refused");
        let msg = format!("{err:#}");
        assert!(
            msg.contains(&format!("{{#{alias}")) && msg.contains("asks nothing"),
            "the refusal names the row: {msg}\n{edited}"
        );
        session.sut.settle_projections();
        assert_eq!(state(), before, "{msg}");
    }
}

/// Without a rollback, a row whose later write fails keeps the writes before
/// it, in the store and in the org file, and the error names them. The failure
/// is injected: a trigger on the SQL authority's table aborts content writes.
#[test]
fn without_a_rollback_a_row_whose_later_write_fails_is_a_disclosed_partial_apply() {
    let (session, page) = Session::boot_sql_only().seed(&[SeedRow {
        state: Some("TODO".to_string()),
        ..SeedRow::plain("Plan")
    }]);
    let (handle, dense) = session.query(&page);
    let sql = |sql: &str| {
        session
            .sut
            .runtime()
            .block_on(session.driver.execute_raw_sql(sql))
            .unwrap_or_else(|e| panic!("{sql}: {e:#}"))
    };
    sql(
        "CREATE TRIGGER dense_patch_fails_content BEFORE UPDATE OF content ON block_raw \
         BEGIN SELECT RAISE(ABORT, 'injected content write failure'); END",
    );
    let edited = replace(&dense, "* TODO Plan {#", "* DONE Renamed {#");
    let err = session
        .patch(&handle, &edited, &[])
        .expect_err("the trigger aborts the content write");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("PARTIAL APPLY: op 2 of 2 failed, and the 1 op(s) before it ARE in the store"),
        "{msg}\n{edited}"
    );
    sql("DROP TRIGGER dense_patch_fails_content");
    session.sut.settle_projections();
    let row = &page.rows[0];
    let stored = sql(&format!(
        "SELECT content, json_extract(properties, '$.task_state') AS state FROM block_raw \
         WHERE id = 'block:{row}'"
    ));
    assert_eq!(
        (&stored["rows"][0]["content"], &stored["rows"][0]["state"]),
        (&serde_json::json!("Plan"), &serde_json::json!("DONE")),
        "the state write landed, the content write did not: {msg}"
    );
    let file = session
        .sut
        .runtime()
        .block_on(
            session
                .driver
                .call_tool_json("read_org_file", serde_json::json!({ "doc_id": page.file })),
        )
        .expect("read_org_file");
    let file = file["content"].as_str().expect("content");
    assert!(
        file.contains("* DONE Plan"),
        "the org file holds what the store holds: {file}"
    );
}

/// A done keyword outside org's defaults is stored done, on an edited row and
/// on a created one, as its file's ring declares.
#[test]
fn a_custom_done_keyword_is_stored_with_its_rings_category() {
    let (session, page) = Session::boot().seed_with(
        &[SeedRow {
            state: Some("TODO".to_string()),
            ..SeedRow::plain("Task")
        }],
        Some(holon_integration_tests::pbt::generators::TodoKeywordSet(
            vec![TaskState::active("TODO"), TaskState::done("SHIPPED")],
        )),
    );
    let (handle, dense) = session.query(&page);
    let edited = format!(
        "{}* SHIPPED Fresh row\n",
        replace(&dense, "* TODO Task {#", "* SHIPPED Task {#")
    );
    let outcome =
        judge(&session, &page, &handle, &edited, &[]).unwrap_or_else(|broken| panic!("{broken}"));
    assert!(outcome.starts_with("reads back exactly"), "{outcome}");
}

/// Org element lines in a row's body. A keyword line (`#+KEY: value`) is an
/// org keyword, not body text, and reads back exactly in its place, store and
/// file; a `#+TODO:` line declares the file's keyword ring, so it is refused by
/// the row's name with the `,#+` hint and nothing is written. The same line
/// written `,#+` is text and reads back exactly. Every other element reads
/// back exactly or is refused by name.
#[test]
fn an_org_element_line_in_a_body_is_refused_by_name_or_is_text() {
    let seed = SeedRow {
        body: vec!["a body line".to_string()],
        ..SeedRow::plain("Alpha")
    };
    let variants: &[(&str, &str)] = &[
        ("caption keyword", "#+CAPTION: a figure"),
        ("todo keyword", "#+TODO: LATER | DONE"),
        ("attr keyword", "#+ATTR_HTML: :width 100"),
        ("keyword after the body", "a body line\n#+NAME: fig"),
        ("escaped keyword", ",#+CAPTION: a figure"),
        ("quote block", "#+begin_quote\nq\n#+end_quote"),
        ("source block", "#+begin_src sh\necho hi\n#+end_src"),
        ("table", "| a | b |"),
        ("drawer", ":NOTE:\nx\n:END:"),
        ("rule", "-----"),
        ("footnote", "[fn:1] a note"),
        ("comment", "# a comment"),
        ("blank line inside", "one\n\ntwo"),
    ];
    let mut session = Session::boot();
    for (name, body) in variants {
        let (next, page) = session.seed(std::slice::from_ref(&seed));
        session = next;
        let (handle, dense) = session.query(&page);
        let edited = replace(&dense, "\na body line", &format!("\n{body}"));
        let outcome = judge(&session, &page, &handle, &edited, &[])
            .unwrap_or_else(|broken| panic!("{name}: {broken}"));
        println!("[element line] {name}: {outcome}");
        if ["caption keyword", "attr keyword", "keyword after the body"].contains(name) {
            assert!(
                outcome.starts_with("reads back exactly"),
                "{name}: a keyword line is written in its place: {outcome}"
            );
        }
        if *name == "todo keyword" {
            assert!(
                outcome.starts_with("refused") && outcome.contains(",#+"),
                "{name}: a ring line is refused with the `,#+` hint: {outcome}"
            );
        }
        if *name == "escaped keyword" {
            assert!(
                outcome.starts_with("reads back exactly"),
                "{name}: {outcome}"
            );
        }
    }
}

/// A keyword line added to a row's body, then removed, and one in a new row's
/// body: each reads back exactly, store and file.
#[test]
fn a_keyword_line_in_a_body_is_written_and_removed() {
    let seed = SeedRow {
        body: vec!["a body line".to_string()],
        ..SeedRow::plain("Alpha")
    };
    let (session, page) = Session::boot().seed(std::slice::from_ref(&seed));
    let (handle, dense) = session.query(&page);
    let added = replace(
        &dense,
        "\na body line",
        "\n#+CAPTION: a figure\na body line",
    );
    let outcome = judge(&session, &page, &handle, &added, &[]).unwrap_or_else(|b| panic!("{b}"));
    assert!(
        outcome.starts_with("reads back exactly"),
        "added: {outcome}"
    );
    let file = org_file_named(&session.sut, &session.driver, &page.file);
    assert!(file.contains("#+CAPTION: a figure\na body line"), "{file}");

    let (handle, dense) = session.query(&page);
    let removed = replace(&dense, "#+CAPTION: a figure\n", "");
    let outcome = judge(&session, &page, &handle, &removed, &[]).unwrap_or_else(|b| panic!("{b}"));
    assert!(
        outcome.starts_with("reads back exactly"),
        "removed: {outcome}"
    );
    let file = org_file_named(&session.sut, &session.driver, &page.file);
    assert!(!file.contains("#+CAPTION"), "{file}");

    let (handle, dense) = session.query(&page);
    let created = format!("{}\n* New row\n#+NAME: fig\nnew body\n", dense.trim_end());
    let outcome = judge(&session, &page, &handle, &created, &[]).unwrap_or_else(|b| panic!("{b}"));
    assert!(
        outcome.starts_with("reads back exactly"),
        "created: {outcome}"
    );
    let file = org_file_named(&session.sut, &session.driver, &page.file);
    assert!(file.contains("#+NAME: fig\nnew body"), "{file}");
}

/// The parser carriers dense_patch writes stay refused on every raw route: an
/// agent's `execute_operation` may not name `_drawer_order` or
/// `_keyword_lines`, and nothing is written.
#[test]
fn a_raw_write_of_a_parsed_carrier_is_refused() {
    let seed = SeedRow {
        body: vec!["a body line".to_string()],
        ..SeedRow::plain("Alpha")
    };
    let (session, page) = Session::boot().seed(std::slice::from_ref(&seed));
    let id = format!("block:{}", page.rows[0]);
    let keyword_lines = r##"[{"before_line":0,"raw":"#+CAPTION: x\n* Forged\n:ID: forged\n"}]"##;
    let writes = [
        (
            "set_field",
            serde_json::json!({ "id": id, "field": "_drawer_order", "value": "[\"b\",\"a\"]" }),
        ),
        (
            "set_field",
            serde_json::json!({ "id": id, "field": "_keyword_lines", "value": keyword_lines }),
        ),
        (
            "create",
            serde_json::json!({
                "id": "block:forged",
                "parent_id": format!("block:{}", page.home),
                "content": "Forged",
                "content_type": "text",
                "_keyword_lines": keyword_lines,
            }),
        ),
    ];
    for (op, params) in writes {
        let before = session.snapshot(&page);
        let refused = session
            .sut
            .runtime()
            .block_on(session.driver.call_tool_json(
                "execute_operation",
                serde_json::json!({ "entity_name": "block", "operation": op, "params": params }),
            ))
            .expect_err("an agent's raw carrier write is refused");
        assert!(
            format!("{refused:#}").contains("REFUSED"),
            "{op}: {refused:#}"
        );
        session.sut.settle_projections();
        assert_eq!(
            session.snapshot(&page),
            before,
            "{op} {params}: nothing is written"
        );
    }
}

/// Text crafted to smuggle file structure through a keyword line — a headline,
/// an `:ID:` line, a drawer or its end marker after it, or a line break inside
/// it — reads back exactly as the rows the text holds, or is refused by name.
/// No block takes the smuggled id, in the store or the file.
#[test]
fn a_crafted_keyword_line_forges_nothing() {
    let seed = SeedRow {
        body: vec!["a body line".to_string()],
        ..SeedRow::plain("Alpha")
    };
    let variants: &[(&str, &str)] = &[
        ("headline after", "#+CAPTION: a\n* Forged headline"),
        ("id line after", "#+CAPTION: a\n:ID: forged-id"),
        (
            "drawer after",
            "#+CAPTION: a\n:PROPERTIES:\n:ID: forged-id\n:END:",
        ),
        ("end marker after", "#+CAPTION: a\n:END:"),
        (
            "drawer markers in the value",
            "#+CAPTION: :END: :ID: forged-id",
        ),
        ("carriage return inside", "#+CAPTION: a\rforged"),
        (
            "headline in the value",
            "#+CAPTION: * Forged :ID: forged-id",
        ),
    ];
    let mut session = Session::boot();
    for (name, body) in variants {
        let (next, page) = session.seed(std::slice::from_ref(&seed));
        session = next;
        let (handle, dense) = session.query(&page);
        let edited = replace(&dense, "\na body line", &format!("\n{body}"));
        let outcome = judge(&session, &page, &handle, &edited, &[])
            .unwrap_or_else(|broken| panic!("{name}: {broken}"));
        println!("[crafted keyword line] {name}: {outcome}");
        let stored = session
            .sut
            .runtime()
            .block_on(
                session
                    .driver
                    .execute_raw_sql("SELECT id FROM block_raw WHERE id LIKE '%forged%'"),
            )
            .expect("read block_raw");
        assert_eq!(stored["rows"], serde_json::json!([]), "{name}: {stored}");
        let file = org_file_named(&session.sut, &session.driver, &page.file);
        let parsed = holon_org_format::parse_org_file(
            std::path::Path::new(&page.file),
            &file,
            &EntityUri::no_parent(),
            std::path::Path::new(""),
        )
        .unwrap_or_else(|e| panic!("{name}: the org file parses: {e:#}\n{file}"));
        assert!(
            parsed
                .blocks
                .iter()
                .all(|b| !b.id.as_str().contains("forged")),
            "{name}: the file holds a smuggled id:\n{file}"
        );
    }
}

/// Org's keyword rule strips a parenthesis group only at the END of a ring
/// word: in `#+TODO: FOO(bar | DONE` the keyword is `FOO(bar`, so `FOO Plan`
/// is title text and `FOO(bar Plan` is the state.
#[test]
fn a_ring_word_with_an_unclosed_parenthesis_is_the_keyword() {
    let (session, page) = Session::boot().seed_with(
        &[SeedRow::plain("Plan"), SeedRow::plain("Other")],
        Some(holon_integration_tests::pbt::generators::TodoKeywordSet(
            vec![TaskState::active("FOO(bar"), TaskState::done("DONE")],
        )),
    );
    let (handle, dense) = session.query(&page);
    assert!(dense.contains("#+TODO: FOO(bar | DONE\n"), "{dense}");
    let edited = replace(
        &replace(&dense, "* Plan {#", "* FOO Plan {#"),
        "* Other {#",
        "* FOO(bar Other {#",
    );
    let outcome =
        judge(&session, &page, &handle, &edited, &[]).unwrap_or_else(|broken| panic!("{broken}"));
    assert!(outcome.starts_with("reads back exactly"), "{outcome}");
    let now = rows(&session.query(&page).1);
    assert!(
        now.iter()
            .any(|r| r.title == "FOO Plan" && r.state.is_none())
            && now
                .iter()
                .any(|r| r.title == "Other" && r.state.as_deref() == Some("FOO(bar")),
        "{now:?}"
    );
}

/// A file's `#+TODO:` keywords changed after dense_query: the patch is a
/// conflict, and nothing is written.
#[test]
fn a_keyword_ring_changed_since_dense_query_is_a_conflict() {
    let (session, page) = Session::boot().seed_with(&[SeedRow::plain("Plan")], next_done_ring());
    let (handle, dense) = session.query(&page);
    let session = session.write_file(
        &page.file,
        page.blocks.clone(),
        Some(holon_integration_tests::pbt::generators::TodoKeywordSet(
            vec![TaskState::active("LATER"), TaskState::done("DONE")],
        )),
    );
    let before = session.snapshot(&page);
    assert!(before.2.contains("#+TODO: LATER | DONE"), "{}", before.2);
    let err = session
        .patch(
            &handle,
            &replace(&dense, "* Plan {#", "* NEXT Plan {#"),
            &[],
        )
        .expect_err("the ring the text was written against is gone");
    let msg = format!("{err:#}");
    assert!(msg.contains("conflict") && msg.contains("#+TODO:"), "{msg}");
    session.sut.settle_projections();
    assert_eq!(
        session.snapshot(&page),
        before,
        "nothing may be written: {msg}"
    );
}

/// A ring spelled with org's fast-access keys (`NEXT(n)`) declares `NEXT`: the
/// keyword applies to the store and the file, and the file keeps its spelling.
#[test]
fn a_fast_access_ring_declares_the_keyword_before_the_key() {
    let (session, page) = Session::boot().seed_with(
        &[SeedRow::plain("Plan")],
        Some(holon_integration_tests::pbt::generators::TodoKeywordSet(
            vec![TaskState::active("NEXT(n)"), TaskState::done("DONE(d)")],
        )),
    );
    let (handle, dense) = session.query(&page);
    assert!(dense.contains("#+TODO: NEXT | DONE\n"), "{dense}");
    let edited = replace(&dense, "* Plan {#", "* NEXT Plan {#");
    let outcome =
        judge(&session, &page, &handle, &edited, &[]).unwrap_or_else(|broken| panic!("{broken}"));
    assert!(outcome.starts_with("reads back exactly"), "{outcome}");
    let file = org_file_named(&session.sut, &session.driver, &page.file);
    assert!(file.contains("#+TODO: NEXT(n) | DONE(d)\n"), "{file}");
}

/// Deleting a row and one of its descendants, with the row between them left
/// out of the projection, plans ONE subtree delete.
#[test]
fn a_delete_inside_another_deleted_subtree_is_not_planned() {
    let (session, _page) = Session::boot().seed(&[
        SeedRow {
            children: vec![SeedRow {
                children: vec![SeedRow::plain("Leaf")],
                ..SeedRow::plain("Middle")
            }],
            ..SeedRow::plain("Top")
        },
        SeedRow::plain("Other"),
    ]);
    let query = "SELECT * FROM block WHERE id IN ('block:pp0-r0', 'block:pp0-r0-c0-c0', \
                 'block:pp0-r1') ORDER BY sort_key";
    let proj = session
        .sut
        .runtime()
        .block_on(session.driver.call_tool_json(
            "dense_query",
            serde_json::json!({ "query": query, "language": "holon_sql" }),
        ))
        .expect("dense_query");
    let dense = proj["dense_org"].as_str().expect("dense_org").to_string();
    let text = DenseRows::split(&dense);
    let alias_of = |title: &str| {
        let row = text
            .rows
            .iter()
            .position(|r| r[0].contains(title))
            .unwrap_or_else(|| panic!("no {title} row:\n{dense}"));
        text.alias(row).expect("token")
    };
    let mut kept = text.clone();
    kept.rows.retain(|r| r[0].contains("Other"));
    let dry = session
        .sut
        .runtime()
        .block_on(session.driver.call_tool_json(
            "dense_patch",
            serde_json::json!({
                "handle": proj["projection_handle"],
                "text": kept.join(),
                "delete": [alias_of("Top"), alias_of("Leaf")],
                "dry_run": true,
            }),
        ))
        .expect("dry run");
    assert_eq!(
        dry["ops"],
        serde_json::json!([{"op": "delete_subtree", "block": "block:pp0-r0"}]),
        "the leaf goes with the top row's subtree: {dry}"
    );
}

/// The headline of the first row titled `title` in `dense`, and its stars.
fn row_stars(dense: &str, title: &str) -> String {
    let headline = dense
        .lines()
        .find(|l| {
            l.trim_start_matches('*')
                .starts_with(&format!(" {title} {{#"))
        })
        .unwrap_or_else(|| panic!("no {title} row:\n{dense}"));
    headline[..headline.len() - headline.trim_start_matches('*').len()].to_string()
}

/// `dense` with new rows at `stars`, each a title and one body line.
fn with_new_rows(dense: &str, stars: &str, rows: &[(String, String)]) -> String {
    let mut text = dense.trim_end_matches('\n').to_string();
    for (title, line) in rows {
        text.push_str(&format!("\n{stars} {title}\n{line}"));
    }
    text.push('\n');
    text
}

/// One patch writes at most `MAX_WRITTEN_ROWS_PER_PATCH` rows: one more is
/// refused by the row crossing the bound, before any write. At the bound,
/// with every row at `MAX_EMPHASIS_MARKS_PER_ROW` marks, it applies.
#[test]
fn a_patch_over_a_work_bound_is_refused_by_the_row_crossing_it() {
    use holon_mcp::dense_patch::MAX_EMPHASIS_MARKS_PER_ROW;
    use holon_mcp::dense_patch::MAX_WRITTEN_ROWS_PER_PATCH;

    let (s, page) = Session::boot().seed(&[SeedRow::plain("Heavy")]);
    s.sut.settle_projections();
    let (handle, dense) = s.query(&page);
    let stars = row_stars(&dense, "Heavy");
    let over: Vec<(String, String)> = (0..=MAX_WRITTEN_ROWS_PER_PATCH)
        .map(|i| (format!("Row{i}"), "plain words".to_string()))
        .collect();
    let edited = with_new_rows(&dense, &stars, &over);
    let before = s.snapshot(&page);
    let msg = format!(
        "{:#}",
        s.patch(&handle, &edited, &[])
            .expect_err("a patch over the written row bound is refused")
    );
    let crossing = format!("Row{MAX_WRITTEN_ROWS_PER_PATCH}");
    assert!(
        msg.contains(&format!("new row \"{stars} {crossing}\"")),
        "the refusal must name {crossing}: {msg}"
    );
    s.sut.settle_projections();
    assert_eq!(s.snapshot(&page), before, "a refused patch writes nothing");

    let worst = vec!["/x"; MAX_EMPHASIS_MARKS_PER_ROW].join(" ");
    let at_bounds: Vec<(String, String)> = (0..MAX_WRITTEN_ROWS_PER_PATCH)
        .map(|i| (format!("Bound{i}"), worst.clone()))
        .collect();
    let (handle, dense) = s.query(&page);
    let edited = with_new_rows(&dense, &stars, &at_bounds);
    let started = std::time::Instant::now();
    s.patch(&handle, &edited, &[])
        .unwrap_or_else(|e| panic!("a patch at both bounds applies: {e:#}"));
    println!(
        "[work-bound] the patch at both bounds took {:?}",
        started.elapsed()
    );
    s.sut.settle_projections();
    let (_, now) = s.query(&page);
    assert_eq!(row_texts(&now), row_texts(&edited), "re-queried:\n{now}");
    let file = s.file_dense(&page);
    assert_eq!(
        row_texts(&file),
        row_texts(&edited),
        "the org file:\n{file}"
    );
}

/// A new row's headline stars are its level, not emphasis marks: a level-2
/// row whose body holds exactly `MAX_EMPHASIS_MARKS_PER_ROW` marks is written.
#[test]
fn a_level_two_row_at_the_mark_bound_is_written() {
    let bound = holon_mcp::dense_patch::MAX_EMPHASIS_MARKS_PER_ROW;
    let (s, page) = Session::boot().seed(&[SeedRow::plain("Parent")]);
    s.sut.settle_projections();
    let (handle, dense) = s.query(&page);
    let stars = format!("{}*", row_stars(&dense, "Parent"));
    assert_eq!(stars, "**", "{dense}");
    let edited = with_new_rows(
        &dense,
        &stars,
        &[("Sub".to_string(), vec!["foo_bar"; bound].join(" "))],
    );
    s.patch(&handle, &edited, &[])
        .unwrap_or_else(|e| panic!("a level-2 row at the bound is written: {e:#}\n{edited}"));
    s.sut.settle_projections();
    let (_, now) = s.query(&page);
    assert_eq!(row_texts(&now), row_texts(&edited), "re-queried:\n{now}");
    let file = s.file_dense(&page);
    assert_eq!(
        row_texts(&file),
        row_texts(&edited),
        "the org file:\n{file}"
    );
}

/// `dense` with its first `count` rows in reverse order; each row is one
/// headline line with no body.
fn with_first_rows_reversed(dense: &str, count: usize) -> String {
    let lines: Vec<&str> = dense.lines().collect();
    let first = lines
        .iter()
        .position(|l| l.starts_with('*'))
        .unwrap_or_else(|| panic!("no row:\n{dense}"));
    let mut rows = lines[first..].to_vec();
    assert!(
        rows.iter().all(|l| l.starts_with("* ")),
        "every row is a top-level headline line:\n{dense}"
    );
    rows[..count].reverse();
    let mut text = lines[..first].to_vec();
    text.extend(rows);
    text.join("\n") + "\n"
}

/// A moved row is a written row: a patch moving more than
/// `MAX_WRITTEN_ROWS_PER_PATCH` rows is refused by the row that crosses the
/// bound, before any write; one moving exactly that many applies.
#[test]
fn a_patch_moving_rows_over_the_written_row_bound_is_refused() {
    use holon_mcp::dense_patch::MAX_WRITTEN_ROWS_PER_PATCH;

    let rows: Vec<SeedRow> = (0..MAX_WRITTEN_ROWS_PER_PATCH + 2)
        .map(|i| SeedRow::plain(&format!("Row{i}")))
        .collect();
    let (s, page) = Session::boot().seed(&rows);
    s.sut.settle_projections();
    let (handle, dense) = s.query(&page);
    let edited = with_first_rows_reversed(&dense, MAX_WRITTEN_ROWS_PER_PATCH + 2);
    let before = s.snapshot(&page);
    let msg = format!(
        "{:#}",
        s.patch(&handle, &edited, &[])
            .expect_err("a patch moving more rows than the bound is refused")
    );
    assert!(
        msg.contains(&format!(
            "this is written row {}",
            MAX_WRITTEN_ROWS_PER_PATCH + 1
        )) && names_a_row(&msg, &edited, &[]),
        "the refusal names the row crossing the bound: {msg}"
    );
    s.sut.settle_projections();
    assert_eq!(s.snapshot(&page), before, "a refused patch writes nothing");

    let edited = with_first_rows_reversed(&dense, MAX_WRITTEN_ROWS_PER_PATCH + 1);
    s.patch(&handle, &edited, &[])
        .unwrap_or_else(|e| panic!("a patch moving rows at the bound applies: {e:#}"));
    s.sut.settle_projections();
    let (_, now) = s.query(&page);
    assert_eq!(row_texts(&now), row_texts(&edited), "re-queried:\n{now}");
    let file = s.file_dense(&page);
    assert_eq!(
        row_texts(&file),
        row_texts(&edited),
        "the org file:\n{file}"
    );
}

/// `blocks` rows as chains 20 rows deep, each chain head a top-level row.
fn chains(blocks: usize) -> Vec<SeedRow> {
    fn chain(title: String, depth: usize) -> SeedRow {
        SeedRow {
            children: if depth > 1 {
                vec![chain(format!("{title}x"), depth - 1)]
            } else {
                Vec::new()
            },
            ..SeedRow::plain(&title)
        }
    }
    (0..blocks.div_ceil(20))
        .map(|c| chain(format!("Head{c}"), 20.min(blocks - 20 * c)))
        .collect()
}

/// The page's top-level rows: a projection handle and its dense text.
fn top_rows(s: &Session, page: &Page) -> (String, String) {
    let proj = s
        .sut
        .runtime()
        .block_on(s.driver.call_tool_json(
            "dense_query",
            serde_json::json!({
                "query": format!(
                    "SELECT * FROM block WHERE parent_id = 'block:{}' ORDER BY sort_key",
                    page.home
                ),
                "language": "holon_sql",
            }),
        ))
        .expect("dense_query over MCP");
    (
        proj["projection_handle"]
            .as_str()
            .expect("handle")
            .to_string(),
        proj["dense_org"].as_str().expect("dense_org").to_string(),
    )
}

/// A patch writing into org documents that hold more than
/// `MAX_DOCUMENT_BLOCKS_PER_PATCH` blocks together is refused by the row that
/// crosses the bound, before any write. The documents count as the patch
/// leaves them: with one row deleted, the same edit applies.
#[test]
fn a_patch_over_the_document_block_bound_is_refused_by_the_row_crossing_it() {
    use holon_mcp::dense_patch::MAX_DOCUMENT_BLOCKS_PER_PATCH;

    let mut rows = chains(MAX_DOCUMENT_BLOCKS_PER_PATCH - 1);
    rows.push(SeedRow::plain("Leaf"));
    let (s, page) = Session::boot().seed(&rows);
    s.sut.settle_projections();
    let blocks = MAX_DOCUMENT_BLOCKS_PER_PATCH + 1;
    assert_eq!(page.blocks.len(), blocks);

    let (handle, dense) = top_rows(&s, &page);
    let edited = replace(&dense, " Head0 {#", " Renamed {#");
    let before = s.snapshot(&page);
    let msg = format!(
        "{:#}",
        s.patch(&handle, &edited, &[])
            .expect_err("a patch over the document block bound is refused")
    );
    let alias = |text: &str, title: &str| {
        let line = text
            .lines()
            .find(|l| l.contains(&format!(" {title} {{#")))
            .unwrap_or_else(|| panic!("no {title} row:\n{text}"));
        holon_org_format::row_alias(line)
            .expect("a row line")
            .expect("an existing row")
    };
    let renamed = alias(&edited, "Renamed");
    assert!(
        msg.contains(&format!("row {{#{renamed}}}")) && msg.contains(&format!("{blocks} blocks")),
        "the refusal names the row and the document's {blocks} blocks: {msg}"
    );
    s.sut.settle_projections();
    assert_eq!(s.snapshot(&page), before, "a refused patch writes nothing");

    let leaf = alias(&edited, "Leaf");
    let edited: String = edited
        .lines()
        .filter(|l| !l.contains(" Leaf {#"))
        .map(|l| format!("{l}\n"))
        .collect();
    s.patch(&handle, &edited, &[leaf.to_string()])
        .unwrap_or_else(|e| panic!("a patch leaving the document at the bound applies: {e:#}"));
    s.sut.settle_projections();
    let (_, now) = top_rows(&s, &page);
    assert_eq!(row_texts(&now), row_texts(&edited), "re-queried:\n{now}");
    let file = org_file_named(&s.sut, &s.driver, &page.file);
    assert!(
        file.contains("* Renamed"),
        "the org file holds the edit:\n{file}"
    );
}

/// The most blocks one page of the holon-pkm vault holds
/// (`Projects/Holon.org`).
const LARGEST_VAULT_PAGE_BLOCKS: usize = 334;

/// The document work bound admits an edit of as many rows as one patch may
/// write on a page as large as the largest vault page.
#[test]
fn an_edit_of_every_admitted_row_on_the_largest_vault_page_applies() {
    use holon_mcp::dense_patch::MAX_WRITTEN_ROWS_PER_PATCH;

    let rows: Vec<SeedRow> = (0..LARGEST_VAULT_PAGE_BLOCKS - 1)
        .map(|i| SeedRow::plain(&format!("Row{i}")))
        .collect();
    let (s, page) = Session::boot().seed(&rows);
    s.sut.settle_projections();
    let (handle, dense) = s.query(&page);
    let edited = (0..MAX_WRITTEN_ROWS_PER_PATCH).fold(dense, |text, i| {
        replace(&text, &format!(" Row{i} {{#"), &format!(" Renamed{i} {{#"))
    });
    s.patch(&handle, &edited, &[])
        .unwrap_or_else(|e| panic!("an edit of {MAX_WRITTEN_ROWS_PER_PATCH} rows applies: {e:#}"));
    s.sut.settle_projections();
    let (_, now) = s.query(&page);
    assert_eq!(row_texts(&now), row_texts(&edited), "re-queried:\n{now}");
    let file = s.file_dense(&page);
    assert_eq!(
        row_texts(&file),
        row_texts(&edited),
        "the org file:\n{file}"
    );
}

/// A text holding more than `MAX_ROWS_PER_TEXT` rows is refused by the row
/// that crosses the bound, before any parser reads it.
#[test]
fn a_text_over_the_row_bound_is_refused_by_the_row_crossing_it() {
    use holon_mcp::dense_patch::MAX_ROWS_PER_TEXT;

    let (s, page) = Session::boot().seed(&[SeedRow::plain("Only")]);
    s.sut.settle_projections();
    let (handle, dense) = s.query(&page);
    let stars = row_stars(&dense, "Only");
    let rows: Vec<(String, String)> = (0..MAX_ROWS_PER_TEXT)
        .map(|i| (format!("New{i}"), "words".to_string()))
        .collect();
    let edited = with_new_rows(&dense, &stars, &rows);
    let before = s.snapshot(&page);
    let msg = format!(
        "{:#}",
        s.patch(&handle, &edited, &[])
            .expect_err("a text over the row bound is refused")
    );
    let crossing = format!("new row \"{stars} New{}\"", MAX_ROWS_PER_TEXT - 1);
    assert!(
        msg.contains(&crossing),
        "the refusal names {crossing}: {msg}"
    );
    s.sut.settle_projections();
    assert_eq!(s.snapshot(&page), before, "a refused patch writes nothing");
}
