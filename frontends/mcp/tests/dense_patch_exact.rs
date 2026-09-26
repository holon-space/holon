//! The planner half of the `dense_patch` contract: an unedited projection
//! plans nothing, and an edit either plans or is refused by name.
//!
//! Every projection here comes from the real `build_projection` over stored
//! blocks, and every edit is text the agent could type. Whether a planned edit
//! reads back exactly is decided against the real store only
//! (`crates/holon-integration-tests/tests/dense_patch_engine_exact.rs`): no
//! model of the store here judges it.
//!
//! Synthetic data only (repo is PUBLIC).

use std::cell::RefCell;
use std::collections::BTreeMap;

use anyhow::Result;
use holon_api::EntityUri;
use holon_api::Tags;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::types::TaskState;
use holon_mcp::dense_patch::MAX_EMPHASIS_MARKS_PER_ROW;
use holon_mcp::dense_patch::PatchOp;
use holon_mcp::dense_patch::PatchPlan;
use holon_mcp::dense_patch::plan_patch;
use holon_mcp::dense_patch::refuse_unparsable_text;
use holon_mcp::dense_projection::Projection;
use holon_mcp::dense_projection::build_projection;
use holon_org_format::OrgBlockExt;
use holon_org_format::ValueCarrier;
use holon_org_format::parse_dense;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::RngAlgorithm;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRng;
use proptest::test_runner::TestRunner;

const PAGE: &str = "page";

fn stored(id: &str, content: &str, tags: &[&str], props: &[(&str, &str)]) -> Block {
    let mut b = Block::new_text(
        EntityUri::block(id),
        EntityUri::block(PAGE),
        content.to_string(),
    );
    b.tags = Tags::from_tag_iter(tags.iter().map(|t| t.to_string()));
    for (k, v) in props {
        b.set_property(*k, Value::String(v.to_string()));
    }
    b
}

fn project(blocks: &[Block]) -> (Projection, String) {
    let built = build_projection(
        blocks.to_vec(),
        &holon_mcp::dense_projection::DocVocabularies::Uniform(
            holon_org_format::TaskKeywordVocabulary::default(),
        ),
    )
    .expect("projection builds");
    (Projection::new("test".into(), &built), built.dense_text)
}

fn plan(projection: &Projection, text: &str) -> Result<PatchPlan> {
    refuse_unparsable_text(projection, text)?;
    plan_patch(projection, text, &parse_dense(text)?, &[])
}

/// Plan `text` against a projection of `blocks`.
fn plan_of(blocks: &[Block], text: &str) -> Result<PatchPlan> {
    let (projection, _) = project(blocks);
    plan(&projection, text)
}

fn refusal(blocks: &[Block], text: &str) -> String {
    let (projection, _) = project(blocks);
    match plan(&projection, text) {
        Ok(p) => panic!("the edit must be refused, but it plans {:?}\n{text}", p.ops),
        Err(e) => format!("{e:#}"),
    }
}

fn append(dense: &str, rows: &str) -> String {
    format!("{}\n{rows}", dense.trim_end())
}

fn edit(dense: &str, from: &str, to: &str) -> String {
    assert!(dense.contains(from), "{from:?} is not in:\n{dense}");
    dense.replacen(from, to, 1)
}

fn assert_names(msg: &str, needles: &[&str]) {
    for needle in needles {
        assert!(
            msg.contains(needle),
            "the refusal must name {needle:?}: {msg}"
        );
    }
}

#[test]
fn an_unedited_tag_shaped_title_plans_nothing() {
    let blocks = [
        stored("a", "Foo :x:", &[], &[]),
        stored("b", "Ratio :a:b:", &[], &[]),
        stored("c", "hmm :P:", &[], &[]),
        stored("d", "Foo :x:", &["y"], &[]),
    ];
    let (projection, dense) = project(&blocks);
    let plan = plan(&projection, &dense).expect("an unedited projection plans");
    assert!(plan.ops.is_empty(), "ops: {:?}\n{dense}", plan.ops);
}

/// A body line of stars with no space after them is text to org (emacs 30.2
/// reads a headline only from stars and a space): the edit plans as an edit
/// of its row, or is refused by the row's name.
#[test]
fn a_body_line_of_stars_without_a_space_is_text() {
    let blocks = [
        stored("a", "Plan\na body line", &[], &[("owner", "me")]),
        stored("b", "Other", &[], &[]),
    ];
    let (projection, dense) = project(&blocks);
    for line in ["*", "**", "*****", "*\t", "*\tTab", "**\tx"] {
        let text = edit(&dense, "a body line", line);
        match plan(&projection, &text) {
            Ok(plan) => assert!(
                plan.ops.iter().any(|op| matches!(
                    op,
                    PatchOp::SetContent { content, .. } if holon_org_format::render_inline_marks(&content.text, &content.marks).lines().any(|l| l == line)
                )),
                "{line:?} plans no body edit of its row: {:?}\n{text}",
                plan.ops
            ),
            Err(e) => assert_names(&format!("{e:#}"), &["row {#0}"]),
        }
    }
}

/// The most emphasis marks any row of `text` holds, its headline stars aside.
fn heaviest_row_marks(text: &str) -> usize {
    let marks = |line: &str| line.chars().filter(|c| "*/_+=~".contains(*c)).count();
    let mut rows: Vec<usize> = Vec::new();
    for line in text.lines() {
        if holon_org_format::is_headline(line) {
            rows.push(marks(line.trim_start_matches('*')));
        } else if let Some(row) = rows.last_mut() {
            *row += marks(line);
        }
    }
    rows.into_iter().max().unwrap_or(0)
}

/// Emphasis marks in text the projection did not show count against the
/// row's bound, wherever in the row they stand; a shown line counts nothing.
#[test]
fn emphasis_marks_over_the_row_bound_are_refused_by_row_name() {
    let line = |n: usize| format!("{}{}", "/*".repeat(n / 2), "_".repeat(n % 2));
    let blocks = [
        stored("a", "Plan\na body line", &[], &[]),
        stored(
            "b",
            &format!("Shown\n{}", line(MAX_EMPHASIS_MARKS_PER_ROW + 1)),
            &[],
            &[],
        ),
    ];
    let (projection, dense) = project(&blocks);
    plan(&projection, &dense).expect("a shown line counts nothing");
    let half = MAX_EMPHASIS_MARKS_PER_ROW / 2;
    let split = edit(
        &dense,
        "a body line",
        &format!(
            "{}\n{}",
            line(half),
            line(MAX_EMPHASIS_MARKS_PER_ROW - half + 1)
        ),
    );
    let bound = MAX_EMPHASIS_MARKS_PER_ROW.to_string();
    assert_names(&refusal(&blocks, &split), &["row {#0}", &bound]);
    let retitled = edit(
        &dense,
        "* Plan",
        &format!("* Plan {}", line(MAX_EMPHASIS_MARKS_PER_ROW + 1)),
    );
    assert_names(&refusal(&blocks, &retitled), &["row {#0}", &bound]);
    let new_row = append(
        &dense,
        &format!("* Fresh\n={}", line(MAX_EMPHASIS_MARKS_PER_ROW)),
    );
    assert_names(
        &refusal(&blocks, &new_row),
        &["new row \"* Fresh\"", &bound],
    );
}

/// A drawer line under a parser carrier's key is refused by its row's name,
/// whatever value the agent wrote.
#[test]
fn a_parser_carrier_key_in_a_drawer_is_refused_by_name() {
    let blocks = [stored("a", "Row", &[], &[])];
    let (_, dense) = project(&blocks);
    for line in [
        ":_keyword_lines: x",
        ":_keyword_lines: []",
        ":_drawer_order: 5",
        ":_drawer_order: x",
    ] {
        let text = append(&dense, &format!("* New\n:PROPERTIES:\n{line}\n:END:"));
        match plan_of(&blocks, &text) {
            Ok(plan) => panic!("{line:?} must be refused, it plans {:?}", plan.ops),
            Err(e) => assert_names(&format!("{e:#}"), &["new row \"New\""]),
        }
    }
}

#[test]
fn a_retitled_tag_shaped_row_plans() {
    let blocks = [stored("a", "Foo :x:", &[], &[])];
    let (_, dense) = project(&blocks);
    plan_of(&blocks, &edit(&dense, "Foo :x:", "Bar :x:")).expect("a retitle plans");
    plan_of(&blocks, &edit(&dense, "Foo :x:", "Foo :x:z:")).expect("a tag edit plans");
}

#[test]
fn a_value_org_cannot_hold_raw_is_carried_as_a_literal() {
    let blocks = [stored("a", "Row", &[], &[("owner", "me")])];
    let (_, dense) = project(&blocks);
    for value in ["", "  abc", "abc  ", "a\nb", "\"The Book\""] {
        let line = format!(":k: {}", ValueCarrier::HeadlineDrawer.encode(value));
        let text = append(&dense, &format!("* New\n:PROPERTIES:\n{line}\n:END:"));
        let plan =
            plan_of(&blocks, &text).unwrap_or_else(|e| panic!("{line:?} must plan: {e:#}\n{text}"));
        assert!(
            plan.ops.iter().any(|op| matches!(
                op,
                PatchOp::Create { attributes, .. } if attributes.properties.get("k").map(String::as_str) == Some(value)
            )),
            "{line:?} must create `k` = {value:?}: {:?}",
            plan.ops
        );
        let text = edit(&dense, ":owner: me", &format!(":owner: me\n{line}"));
        plan_of(&blocks, &text).unwrap_or_else(|e| panic!("{line:?} must plan: {e:#}"));
    }
}

#[test]
fn a_drawer_line_without_a_value_is_the_empty_value() {
    let blocks = [stored("a", "Row", &[], &[("owner", "me")])];
    let (_, dense) = project(&blocks);
    for line in [":k:", ":k:    "] {
        let text = append(&dense, &format!("* New\n:PROPERTIES:\n{line}\n:END:"));
        let plan =
            plan_of(&blocks, &text).unwrap_or_else(|e| panic!("{line:?} must plan: {e:#}\n{text}"));
        assert!(
            plan.ops.iter().any(|op| matches!(
                op,
                PatchOp::Create { attributes, .. } if attributes.properties.get("k").map(String::as_str) == Some("")
            )),
            "{line:?} must create `k` = \"\": {:?}",
            plan.ops
        );
    }
    for line in [":owner:", ":owner:    "] {
        let text = edit(&dense, ":owner: me", line);
        plan_of(&blocks, &text).unwrap_or_else(|e| panic!("{line:?} must plan: {e:#}"));
    }
}

#[test]
fn an_id_line_names_the_row_or_is_refused() {
    let blocks = [stored("a-1", "Row", &[], &[("owner", "me")])];
    let (projection, dense) = project(&blocks);
    let own = edit(&dense, ":owner: me", ":ID: a-1\n:owner: me");
    let plan = plan(&projection, &own).expect("the row's own id plans");
    assert!(plan.ops.is_empty(), "ops: {:?}", plan.ops);

    for spelling in ["ID", "id", "Id"] {
        let forged = edit(
            &dense,
            ":owner: me",
            &format!(":{spelling}: forged\n:owner: me"),
        );
        assert_names(&refusal(&blocks, &forged), &["row {#0}", "forged"]);
        let text = append(
            &dense,
            &format!("* New\n:PROPERTIES:\n:{spelling}: v\n:END:"),
        );
        assert_names(&refusal(&blocks, &text), &["new row \"New\"", "ID"]);
    }
}

#[test]
fn a_drawer_key_org_does_not_write_back_is_refused() {
    let blocks = [stored("a", "Row", &[], &[("owner", "me")])];
    let (_, dense) = project(&blocks);
    for (key, value) in [
        ("sequence", "9"),
        ("level", "3"),
        ("org_properties", "x"),
        ("TODO", "x"),
        ("SCHEDULED", "x"),
        ("DEADLINE", "x"),
        ("COLLAPSED", "probeval"),
        ("Collapsed", "probeval"),
        ("tags", "probeval"),
        ("TAGS", "probeval"),
        ("Tags", "probeval"),
        ("_x", "v"),
        ("content", "x"),
        ("parent_id", "block:a"),
    ] {
        let line = format!(":{key}: {value}");
        let text = append(&dense, &format!("* New\n:PROPERTIES:\n{line}\n:END:"));
        assert_names(&refusal(&blocks, &text), &["new row \"New\"", key]);
        let text = edit(&dense, ":owner: me", &format!(":owner: me\n{line}"));
        assert_names(&refusal(&blocks, &text), &["row {#0}", key]);
    }
}

#[test]
fn a_column_name_in_another_casing_is_an_ordinary_property() {
    let blocks = [stored("a", "Row", &[], &[])];
    let (_, dense) = project(&blocks);
    for key in ["Content", "CONTENT", "Parent_Id"] {
        let text = append(&dense, &format!("* New\n:PROPERTIES:\n:{key}: x\n:END:"));
        plan_of(&blocks, &text).unwrap_or_else(|e| panic!("`{key}` must plan: {e:#}"));
    }
}

#[test]
fn a_repeated_drawer_key_is_refused() {
    let blocks = [stored("a", "Row", &[], &[("owner", "me")])];
    let (_, dense) = project(&blocks);
    let text = append(
        &dense,
        "* New\n:PROPERTIES:\n:k: line one\n:k: line two\n:END:",
    );
    assert_names(&refusal(&blocks, &text), &["new row \"New\"", "`k`"]);
    let text = edit(&dense, ":owner: me", ":owner: me\n:owner: you");
    assert_names(&refusal(&blocks, &text), &["row {#0}", "`owner`"]);
}

#[test]
fn a_tag_org_cannot_carry_is_refused() {
    let blocks = [stored("a", "Tagged child", &["alpha", "beta"], &[])];
    let (_, dense) = project(&blocks);
    let text = append(&dense, "* Row t :a b:");
    assert_names(&refusal(&blocks, &text), &["new row", "a b"]);
    let text = edit(&dense, ":alpha:beta:", ":a b:");
    assert_names(&refusal(&blocks, &text), &["row {#0}", "a b"]);
}

#[test]
fn an_org_trimmed_value_plans() {
    let blocks = [stored("a", "Row", &[], &[("owner", "  padded  ")])];
    let (projection, dense) = project(&blocks);
    let plan = plan(&projection, &dense).expect("an unedited projection plans");
    assert!(plan.ops.is_empty(), "ops: {:?}\n{dense}", plan.ops);
    let text = append(&dense, "* New\n:PROPERTIES:\n:k:   abc   \n:END:");
    plan_of(&blocks, &text).expect("org reads the value as `abc`");
}

fn with_drawer_order(mut block: Block, keys: &[&str]) -> Block {
    block.set_property(
        holon_org_format::org_props::DRAWER_ORDER,
        Value::String(serde_json::to_string(keys).expect("keys serialize")),
    );
    block
}

#[test]
fn a_drawer_reorder_plans() {
    let blocks = [stored("a", "Row", &[], &[("alpha", "1"), ("zeta", "2")])];
    let (_, dense) = project(&blocks);
    let swapped = plan_of(
        &blocks,
        &edit(&dense, ":alpha: 1\n:zeta: 2", ":zeta: 2\n:alpha: 1"),
    )
    .expect("a reorder plans");
    assert!(!swapped.ops.is_empty(), "a reorder is an edit");
    plan_of(
        &blocks,
        &edit(&dense, ":alpha: 1\n:zeta: 2", ":zeta: 2\n:alpha: 9"),
    )
    .expect("a reorder with a value change plans");

    let authored = [with_drawer_order(
        stored("a", "Row", &[], &[("alpha", "1"), ("zeta", "2")]),
        &["zeta", "alpha"],
    )];
    let (projection, dense) = project(&authored);
    assert!(
        dense.contains(":zeta: 2\n:alpha: 1"),
        "the authored order is shown:\n{dense}"
    );
    let unedited = plan(&projection, &dense).expect("an unedited projection plans");
    assert!(unedited.ops.is_empty(), "ops: {:?}", unedited.ops);
    plan_of(
        &authored,
        &edit(&dense, ":zeta: 2\n:alpha: 1", ":alpha: 1\n:zeta: 2"),
    )
    .expect("a reorder back to key order plans");
    plan_of(
        &authored,
        &edit(
            &dense,
            ":zeta: 2\n:alpha: 1",
            ":zeta: 2\n:mid: 5\n:alpha: 1",
        ),
    )
    .expect("a line inserted mid-drawer plans");

    let text = append(&dense, "* New\n:PROPERTIES:\n:zeta: 1\n:alpha: 2\n:END:");
    plan_of(&authored, &text).expect("a new row keeps its drawer order");
}

#[test]
fn a_stored_property_org_cannot_spell_is_disclosed() {
    let spelled = [("Effort", "3"), ("owner", "me")];
    let unspelled = ["END", "a b", "a:b", "id", "k+"];
    let props: Vec<(&str, &str)> = spelled
        .iter()
        .copied()
        .chain(unspelled.iter().map(|k| (*k, "v")))
        .collect();
    let blocks = [
        stored("a", "Row", &[], &props),
        stored("b", "Plain row", &[], &spelled),
    ];
    let built = build_projection(
        blocks.to_vec(),
        &holon_mcp::dense_projection::DocVocabularies::Uniform(
            holon_org_format::TaskKeywordVocabulary::default(),
        ),
    )
    .expect("projection builds");
    let alias = |id: &str| {
        built
            .alias_table
            .alias_of(&EntityUri::block(id))
            .expect("projected")
            .clone()
    };
    for (key, _) in spelled {
        assert!(
            built.dense_text.contains(&format!(":{key}: ")),
            "`{key}` is shown:\n{}",
            built.dense_text
        );
    }
    let mut expected: Vec<String> = unspelled.iter().map(|k| k.to_string()).collect();
    expected.sort();
    assert_eq!(
        built.omitted,
        BTreeMap::from([(alias("a"), expected)]),
        "every stored key the drawer does not show is disclosed, and only those:\n{}",
        built.dense_text
    );
}

// ---------------------------------------------------------------------------
// Properties over generated stores and edits.
// ---------------------------------------------------------------------------

fn title() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => safe_title(),
        1 => "[A-Za-z][a-z]{0,5} \\{#[0-9]\\}",
        1 => "[A-Za-z] :[a-z] [a-z]:",
        1 => "(TODO|DONE) [a-z]{1,5}",
    ]
}

/// Titles whose text org writes back as it stands.
fn safe_title() -> impl Strategy<Value = String> {
    prop_oneof![
        "[A-Za-z][a-z0-9 ]{0,8}",
        "[A-Za-z][a-z]{0,5} :[a-z]{1,3}:",
        "[A-Za-z][a-z]{0,5} :[a-z]{1,3}:[a-z]{1,3}:",
        "[A-Za-z][a-z]{0,5}:",
        "[A-Za-z][a-z]{0,5} ::[a-z]{1,3}",
        "[A-Za-z][a-z]{0,4} #[a-z]{1,3}",
    ]
}

/// Body lines under a title, among them a line org reads as a headline.
fn body_line() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[a-z][a-z ]{0,8}",
        1 => "- [a-z]{1,4}",
        1 => ":[a-z]{1,3}: [a-z]{1,3}",
        1 => "  [a-z]{1,4}",
        1 => "[a-z]{1,3} \\{#[0-9]\\}",
        1 => Just(String::new()),
        1 => "\\*{1,2} [a-z]{1,4}",
        1 => "\\*{1,5}(\t[a-z]{0,3})?",
        1 => "x ={2,120}",
        1 => "/{2,120} x",
        1 => "\\*{6,120}",
        1 => "[a-z] \\*[a-z]{1,4}\\* [a-z]",
    ]
}

/// Whether `line` is a headline to org (emacs 30.2): stars, then a space.
fn is_org_headline(line: &str) -> bool {
    let rest = line.trim_start_matches('*');
    rest.len() < line.len() && rest.starts_with(' ')
}

/// Whether `content` holds a body line org reads as a headline.
fn has_headline_body(content: &str) -> bool {
    content.lines().skip(1).any(is_org_headline)
}

/// Whether `content` holds a body line of stars org reads as text.
fn has_star_text_body(content: &str) -> bool {
    content
        .lines()
        .skip(1)
        .any(|line| line.starts_with('*') && !is_org_headline(line))
}

/// A stored title with, sometimes, body lines after it.
fn content(title: impl Strategy<Value = String>) -> impl Strategy<Value = String> {
    (title, prop::collection::vec(body_line(), 0..3)).prop_map(|(title, body)| {
        std::iter::once(title)
            .chain(body)
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// Hidden and reserved keys a store can hold, and keys org cannot spell.
const RESERVED_KEYS: &[&str] = &[
    "_note",
    "sequence",
    "level",
    "ID",
    "id",
    "a b",
    "a:b",
    "k+",
    "END",
    "PROPERTIES",
];
/// Block column names in other casings: ordinary properties.
const COLUMN_CASINGS: &[&str] = &["Content", "CONTENT", "Parent_Id", "Sort_Key", "Id"];

/// Property keys no store field or org rule claims.
fn safe_key() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "q[a-z0-9_]{0,4}",
        1 => prop::sample::select(vec!["Effort", "Content", "CONTENT", "Parent_Id", "a+b", "x-y"])
            .prop_map(str::to_string),
    ]
}

fn stored_key() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => "p[a-z_]{0,5}",
        1 => prop::sample::select(RESERVED_KEYS).prop_map(str::to_string),
        1 => prop::sample::select(COLUMN_CASINGS).prop_map(str::to_string),
    ]
}

fn tag() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z]{1,4}",
        "[A-Z][a-z]{0,3}",
        "@[a-z]{1,3}",
        "[a-z]{1,2}_[a-z]{1,2}"
    ]
}

fn key() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => "[a-z][a-z_]{0,6}",
        1 => "[A-Z][A-Za-z]{0,6}",
        4 => prop::sample::select(vec![
            "owner", "Effort", "choose", "Content", "a+b", "ID", "id", "sequence", "level",
            "tags", "Tags", "TAGS", "TODO", "COLLAPSED", "priority", "REQUIRES", "_x", "content",
            "parent_id", "org_properties", "SCHEDULED", "k+", "END", "_keyword_lines",
            "_drawer_order", "item", "Closed",
        ])
        .prop_map(str::to_string),
        3 => prop::sample::select(COLUMN_CASINGS).prop_map(str::to_string),
    ]
}

fn value() -> impl Strategy<Value = String> {
    prop_oneof![
        "[a-z0-9]{1,6}",
        "[ -~]{0,10}",
        "\\PC{0,6}",
        prop::sample::select(vec![
            "",
            " ",
            "a\nb",
            "\"q\"",
            "t",
            "nil",
            "b {#2} :x:",
            ":y:"
        ])
        .prop_map(str::to_string),
    ]
}

#[derive(Clone, Debug)]
struct StoredRow {
    content: String,
    tags: Vec<String>,
    props: Vec<(String, String)>,
    /// A permutation of the property keys, stored as the authored order.
    order: Option<Vec<String>>,
    state: bool,
}

fn stored_row() -> impl Strategy<Value = StoredRow> {
    stored_row_with(title())
}

fn stored_row_with(
    title: impl Strategy<Value = String> + 'static,
) -> impl Strategy<Value = StoredRow> {
    (
        content(title),
        prop::collection::vec(tag(), 0..3),
        prop::collection::btree_map(stored_key(), value(), 0..4),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_flat_map(|(content, tags, props, ordered, state)| {
            let keys: Vec<String> = props.keys().cloned().collect();
            let order = if ordered && keys.len() > 1 {
                Just(keys).prop_shuffle().prop_map(Some).boxed()
            } else {
                Just(None).boxed()
            };
            (
                Just(content),
                Just(tags),
                Just(props.into_iter().collect::<Vec<_>>()),
                order,
                Just(state),
            )
        })
        .prop_map(|(content, tags, props, order, state)| StoredRow {
            content,
            tags,
            props,
            order,
            state,
        })
}

fn store(rows: &[StoredRow]) -> Vec<Block> {
    rows.iter()
        .enumerate()
        .map(|(i, r)| {
            let tags: Vec<&str> = r.tags.iter().map(String::as_str).collect();
            let props: Vec<(&str, &str)> = r
                .props
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let mut b = stored(&format!("s{i}"), &r.content, &tags, &props);
            if let Some(order) = &r.order {
                let keys: Vec<&str> = order.iter().map(String::as_str).collect();
                b = with_drawer_order(b, &keys);
            }
            if r.state {
                b.set_task_state(Some(TaskState::active("TODO")));
            }
            b
        })
        .collect()
}

#[derive(Clone, Debug)]
enum RowEdit {
    Retitle(String),
    Retag(Vec<String>),
    /// Set a drawer line: replace the line of its key, or add one.
    AddLine(String, String),
    RawLine(String),
    DropLine(usize),
    /// Move drawer line `from` to position `to`.
    MoveLine(usize, usize),
    NewRow(String, Vec<String>, Vec<(String, String)>),
}

fn row_edit() -> impl Strategy<Value = RowEdit> {
    prop_oneof![
        1 => title().prop_map(RowEdit::Retitle),
        1 => prop::collection::vec(tag(), 0..3).prop_map(RowEdit::Retag),
        2 => (key(), value()).prop_map(|(k, v)| RowEdit::AddLine(k, v)),
        1 => "[ -~]{0,12}".prop_map(RowEdit::RawLine),
        1 => (0usize..4).prop_map(RowEdit::DropLine),
        1 => (0usize..4, 0usize..4).prop_map(|(a, b)| RowEdit::MoveLine(a, b)),
        1 => (
            title(),
            prop::collection::vec(tag(), 0..3),
            prop::collection::vec((key(), value()), 0..3),
        )
            .prop_map(|(t, g, p)| RowEdit::NewRow(t, g, p)),
    ]
}

/// Edits org writes back as they stand.
fn safe_edit() -> impl Strategy<Value = RowEdit> {
    prop_oneof![
        safe_title().prop_map(RowEdit::Retitle),
        prop::collection::vec(tag(), 0..3).prop_map(RowEdit::Retag),
        (safe_key(), value()).prop_map(|(k, v)| RowEdit::AddLine(k, v)),
        (0usize..4).prop_map(RowEdit::DropLine),
        (0usize..4, 0usize..4).prop_map(|(a, b)| RowEdit::MoveLine(a, b)),
        (
            safe_title(),
            prop::collection::vec(tag(), 0..3),
            prop::collection::btree_map(safe_key(), value(), 0..3),
        )
            .prop_map(|(t, g, p)| RowEdit::NewRow(t, g, p.into_iter().collect())),
    ]
}

fn drawer_line(key: &str, value: &str) -> String {
    format!(":{key}: {}", ValueCarrier::HeadlineDrawer.encode(value))
}

fn tag_group(tags: &[String]) -> String {
    if tags.is_empty() {
        String::new()
    } else {
        format!(" :{}:", tags.join(":"))
    }
}

/// `dense` with row `row` (0-based among headlines) edited by `e`.
fn apply_edit(dense: &str, row: usize, e: &RowEdit) -> String {
    let lines: Vec<&str> = dense.lines().collect();
    let heads: Vec<usize> = (0..lines.len())
        .filter(|&i| is_org_headline(lines[i]))
        .collect();
    if heads.is_empty() {
        return dense.to_string();
    }
    let at = heads[row % heads.len()];
    let end = heads
        .iter()
        .copied()
        .find(|&h| h > at)
        .unwrap_or(lines.len());
    let head = lines[at];
    let token = &head[head.rfind(" {#").expect("an existing row carries a token")..];
    let mut out: Vec<String> = lines[..at].iter().map(|l| l.to_string()).collect();
    let mut body: Vec<String> = lines[at + 1..end].iter().map(|l| l.to_string()).collect();
    let insert_line =
        |body: &mut Vec<String>, line: String| match body.iter().position(|l| l == ":END:") {
            Some(end) => body.insert(end, line),
            None => {
                body.insert(0, ":END:".to_string());
                body.insert(0, line);
                body.insert(0, ":PROPERTIES:".to_string());
            }
        };
    let drawer_lines = |body: &[String]| -> Vec<usize> {
        let Some(end) = body.iter().position(|l| l == ":END:") else {
            return Vec::new();
        };
        (0..end)
            .filter(|&i| body[i].starts_with(':') && body[i] != ":PROPERTIES:")
            .collect()
    };
    let mut head = head.to_string();
    match e {
        RowEdit::Retitle(t) => head = format!("* {t}{token}"),
        RowEdit::Retag(tags) => {
            let (title, _) =
                holon_org_format::parser::split_headline_tags(&head[2..head.len() - token.len()]);
            head = format!("* {title}{}{token}", tag_group(tags));
        }
        RowEdit::AddLine(k, v) => {
            let line = drawer_line(k, v);
            let prefix = format!(":{k}:");
            match drawer_lines(&body)
                .into_iter()
                .find(|&i| body[i].starts_with(&prefix))
            {
                Some(i) => body[i] = line,
                None => insert_line(&mut body, line),
            }
        }
        RowEdit::RawLine(raw) => insert_line(&mut body, raw.clone()),
        RowEdit::DropLine(n) => {
            let props = drawer_lines(&body);
            if !props.is_empty() {
                body.remove(props[n % props.len()]);
            }
        }
        RowEdit::MoveLine(from, to) => {
            let props = drawer_lines(&body);
            if !props.is_empty() {
                let line = body.remove(props[from % props.len()]);
                body.insert(props[to % props.len()], line);
            }
        }
        RowEdit::NewRow(..) => {}
    }
    out.push(head);
    out.extend(body);
    out.extend(lines[end..].iter().map(|l| l.to_string()));
    if let RowEdit::NewRow(t, tags, props) = e {
        out.push(format!("* {t}{}", tag_group(tags)));
        if !props.is_empty() {
            out.push(":PROPERTIES:".to_string());
            out.extend(props.iter().map(|(k, v)| drawer_line(k, v)));
            out.push(":END:".to_string());
        }
    }
    out.join("\n") + "\n"
}

/// The rows of a dense text, each as its lines, headline first.
fn text_rows(text: &str) -> Vec<Vec<&str>> {
    let mut rows: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        match rows.last_mut() {
            Some(row) if !is_org_headline(line) => row.push(line),
            _ if is_org_headline(line) => rows.push(vec![line]),
            _ => {}
        }
    }
    rows
}

/// Whether the edit puts two drawer keys equal in any case into a row it
/// changes, one of their lines new to that row: org reads them as one
/// ambiguous property, so dense_patch refuses that row. A pair the store
/// already held in a row the edit changes, or in a row it leaves, is no such
/// edit.
fn edits_in_a_case_pair(dense: &str, text: &str) -> bool {
    let before = text_rows(dense);
    let token = |row: &[&str]| row[0].rfind(" {#").map(|at| row[0][at..].to_string());
    text_rows(text).into_iter().any(|row| {
        let shown: &[&str] = token(&row)
            .and_then(|t| before.iter().find(|b| token(b).as_ref() == Some(&t)))
            .map(Vec::as_slice)
            .unwrap_or_default();
        if row == shown {
            return false;
        }
        let drawer: Vec<&str> = match (
            row.iter().position(|l| *l == ":PROPERTIES:"),
            row.iter().position(|l| *l == ":END:"),
        ) {
            (Some(open), Some(end)) => row[open + 1..end].to_vec(),
            _ => Vec::new(),
        };
        let key = |line: &str| {
            line.strip_prefix(':')
                .and_then(|l| l.split(':').next())
                .unwrap_or_else(|| panic!("a drawer line holds `:key:`, got {line:?}"))
                .to_lowercase()
        };
        drawer.iter().enumerate().any(|(i, a)| {
            drawer[..i]
                .iter()
                .any(|b| key(a) == key(b) && !(shown.contains(a) && shown.contains(b)))
        })
    })
}

fn refusal_names_a_row(msg: &str) -> bool {
    msg.contains("row {#") || msg.contains("new row")
}

/// The risky shapes a store holds, by name.
fn stored_reach(rows: &[StoredRow]) -> Vec<&'static str> {
    let mut hit = Vec::new();
    let keys = || {
        rows.iter()
            .flat_map(|r| r.props.iter().map(|(k, _)| k.as_str()))
    };
    if keys().any(|k| RESERVED_KEYS.contains(&k)) {
        hit.push("stored reserved or unspellable key");
    }
    if keys().any(|k| COLUMN_CASINGS.contains(&k)) {
        hit.push("stored column name in another casing");
    }
    if rows.iter().any(|r| r.content.contains('\n')) {
        hit.push("stored multi-line content");
    }
    if rows.iter().any(|r| r.order.is_some()) {
        hit.push("stored authored drawer order");
    }
    if rows.iter().any(|r| has_headline_body(&r.content)) {
        hit.push("stored body line that reads as a row");
    }
    if rows.iter().any(|r| has_star_text_body(&r.content)) {
        hit.push("stored body line of stars org reads as text");
    }
    if rows
        .iter()
        .any(|r| !r.state && (r.content.starts_with("TODO ") || r.content.starts_with("DONE ")))
    {
        hit.push("stored keyword-headed title with no state");
    }
    hit
}

/// The risky shapes an edit holds, by name.
fn edit_reach(e: &RowEdit) -> Vec<&'static str> {
    let mut hit = Vec::new();
    let keys: Vec<&str> = match e {
        RowEdit::AddLine(k, _) => vec![k.as_str()],
        RowEdit::NewRow(_, _, props) => props.iter().map(|(k, _)| k.as_str()).collect(),
        _ => Vec::new(),
    };
    let values: Vec<&str> = match e {
        RowEdit::AddLine(_, v) => vec![v.as_str()],
        RowEdit::NewRow(_, _, props) => props.iter().map(|(_, v)| v.as_str()).collect(),
        _ => Vec::new(),
    };
    if keys
        .iter()
        .any(|k| k.starts_with('_') || holon_org_format::models::is_hidden_drawer_key(k))
    {
        hit.push("edit with a reserved key");
    }
    if keys.iter().any(|k| COLUMN_CASINGS.contains(k)) {
        hit.push("edit with a column name in another casing");
    }
    if values
        .iter()
        .any(|v| ValueCarrier::HeadlineDrawer.encode(v) != *v)
    {
        hit.push("edit with a value that needs the literal");
    }
    if matches!(e, RowEdit::MoveLine(..)) {
        hit.push("edit that moves a drawer line");
    }
    hit
}

/// Run `test` over 512 seeded draws of `strategy`, then print how often each
/// named shape was drawn in a case the test did not reject, and require every
/// one at least 10 times.
fn run_property<S: Strategy>(
    name: &str,
    strategy: S,
    reach: impl Fn(&S::Value) -> Vec<&'static str>,
    shapes: &[&'static str],
    test: impl Fn(S::Value) -> Result<(), TestCaseError>,
) {
    let counts: RefCell<BTreeMap<&'static str, usize>> = RefCell::new(BTreeMap::new());
    // A seeded draw: the reach floor is a property of the generator, checked
    // on the same 512 cases every run, so it cannot flake.
    let mut runner = TestRunner::new_with_rng(
        Config {
            cases: 512,
            failure_persistence: None,
            ..Config::default()
        },
        TestRng::deterministic_rng(RngAlgorithm::ChaCha),
    );
    let result = runner.run(&strategy, |value| {
        let hits = reach(&value);
        let outcome = test(value);
        if !matches!(outcome, Err(TestCaseError::Reject(_))) {
            for shape in hits {
                *counts.borrow_mut().entry(shape).or_default() += 1;
            }
        }
        outcome
    });
    let counts = counts.into_inner();
    println!("[reach] {name} over 512 cases: {counts:?}");
    if let Err(e) = result {
        panic!("{name}: {e}");
    }
    for shape in shapes {
        assert!(
            counts.get(shape).copied().unwrap_or(0) >= 10,
            "{name}: {shape:?} was drawn fewer than 10 times: {counts:?}"
        );
    }
}

/// The stored shapes a projection shows faithfully.
const FAITHFUL_STORED_SHAPES: &[&str] = &[
    "stored reserved or unspellable key",
    "stored column name in another casing",
    "stored multi-line content",
    "stored authored drawer order",
];

#[test]
fn an_unedited_projection_plans_nothing() {
    run_property(
        "an_unedited_projection_plans_nothing",
        prop::collection::vec(stored_row(), 1..6),
        |rows| stored_reach(rows),
        &[
            FAITHFUL_STORED_SHAPES,
            &[
                "stored body line that reads as a row",
                "stored body line of stars org reads as text",
                "stored keyword-headed title with no state",
            ],
        ]
        .concat(),
        |rows| {
            let blocks = store(&rows);
            let (projection, dense) = project(&blocks);
            // A `*` body line is shown with one more comma (D230.a), so
            // every stored shape reads back as its row.
            match plan(&projection, &dense) {
                Ok(plan) => {
                    prop_assert!(plan.ops.is_empty(), "ops: {:?}\n{}", plan.ops, dense);
                }
                Err(err) => {
                    prop_assert!(
                        false,
                        "an unedited projection plans nothing: {err:#}\n{dense}"
                    );
                }
            }
            Ok(())
        },
    );
}

#[test]
fn an_edit_plans_or_is_refused_by_name() {
    run_property(
        "an_edit_plans_or_is_refused_by_name",
        (
            prop::collection::vec(stored_row(), 1..5),
            0usize..5,
            row_edit(),
        ),
        |(rows, _, e)| {
            let mut hit = stored_reach(rows);
            hit.extend(edit_reach(e));
            hit
        },
        &[
            FAITHFUL_STORED_SHAPES,
            &[
                "stored keyword-headed title with no state",
                "edit with a reserved key",
                "edit with a column name in another casing",
                "edit with a value that needs the literal",
                "edit that moves a drawer line",
            ],
        ]
        .concat(),
        |(rows, row, e)| {
            let blocks = store(&rows);
            let (projection, dense) = project(&blocks);
            let text = apply_edit(&dense, row, &e);
            let Ok(parse) = parse_dense(&text) else {
                return Ok(());
            };
            match plan_patch(&projection, &text, &parse, &[]) {
                Ok(_) => {}
                Err(err) => {
                    let msg = format!("{err:#}");
                    prop_assert!(
                        refusal_names_a_row(&msg),
                        "the refusal names no row: {}\n{}",
                        msg,
                        text
                    );
                }
            }
            Ok(())
        },
    );
}

#[test]
fn an_edit_org_writes_back_plans() {
    run_property(
        "an_edit_org_writes_back_plans",
        (
            prop::collection::vec(stored_row_with(safe_title()), 1..5),
            0usize..5,
            safe_edit(),
        ),
        |(rows, _, e)| {
            let mut hit = stored_reach(rows);
            hit.extend(edit_reach(e));
            hit
        },
        &[
            FAITHFUL_STORED_SHAPES,
            &[
                "edit with a column name in another casing",
                "edit with a value that needs the literal",
                "edit that moves a drawer line",
            ],
        ]
        .concat(),
        |(rows, row, e)| {
            let blocks = store(&rows);
            let (_, dense) = project(&blocks);
            let text = apply_edit(&dense, row, &e);
            prop_assume!(!edits_in_a_case_pair(&dense, &text));
            prop_assume!(heaviest_row_marks(&text) <= MAX_EMPHASIS_MARKS_PER_ROW);
            if let Err(err) = plan_of(&blocks, &text) {
                prop_assert!(
                    false,
                    "an edit org writes back was refused: {:#}\n{}",
                    err,
                    text
                );
            }
            Ok(())
        },
    );
}

#[test]
fn a_token_that_is_not_trailing_is_refused() {
    let blocks = [
        stored("a", "Row", &[], &[("owner", "me")]),
        stored("b", "First", &[], &[]),
    ];
    let (_, dense) = project(&blocks);
    for (from, to, token) in [
        ("* Row {#0}", "* Row {#0}:urgent:", "{#0}"),
        ("* First {#1}", "* First {#1} tail", "{#1}"),
    ] {
        let msg = refusal(&blocks, &edit(&dense, from, to));
        assert_names(&msg, &[token]);
    }
}

#[test]
fn text_before_the_first_row_is_refused() {
    let blocks = [
        stored("a", "First", &[], &[]),
        stored("b", "Second", &[], &[]),
    ];
    let (_, dense) = project(&blocks);
    let msg = refusal(
        &blocks,
        &edit(&dense, "* First {#0}", " * Renamed first {#0}"),
    );
    assert_names(&msg, &["Renamed first"]);
}

/// A body line starting with `*` is shown with one more comma (D230.a), so
/// the unchanged projection plans nothing.
#[test]
fn a_body_line_starting_with_a_star_plans_nothing_unchanged() {
    let blocks = [stored("a", "Head\n* Bar", &[], &[("owner", "me")])];
    let (projection, dense) = project(&blocks);
    assert!(dense.contains("\n,* Bar\n"), "{dense}");
    let plan = plan(&projection, &dense).expect("an unedited projection plans");
    assert!(plan.ops.is_empty(), "ops: {:?}\n{dense}", plan.ops);
}

/// A store row with no state whose title starts with a keyword shows as a
/// task: unedited it is not an edit, edited it cannot read back as shown.
#[test]
fn a_keyword_headed_title_with_no_stored_state() {
    let blocks = [stored("a", "TODO Plan", &[], &[])];
    let (projection, dense) = project(&blocks);
    let unchanged = plan(&projection, &dense).expect("an unedited row plans");
    assert!(
        unchanged.ops.is_empty(),
        "ops: {:?}\n{dense}",
        unchanged.ops
    );
    let msg = refusal(&blocks, &edit(&dense, "Plan {#0}", "Plan renamed {#0}"));
    assert_names(&msg, &["{#0}", "TODO"]);
}

/// `dense_query` names a row whose text does not show the block as stored:
/// a keyword-headed title stored with no task state reads back as a task.
#[test]
fn a_row_shown_differently_from_its_store_is_disclosed() {
    let blocks = [stored("a", "TODO Plan\na body line", &[], &[])];
    let built = build_projection(
        blocks.to_vec(),
        &holon_mcp::dense_projection::DocVocabularies::Uniform(
            holon_org_format::TaskKeywordVocabulary::default(),
        ),
    )
    .expect("projection builds");
    let why = built
        .unfaithful
        .values()
        .next()
        .unwrap_or_else(|| panic!("the row must be disclosed:\n{}", built.dense_text));
    assert!(
        why.contains("task state"),
        "the disclosure must name the task state: {why}"
    );

    let mut tasked = stored("b", "Plan\na body line", &[], &[]);
    tasked.set_task_state(Some(TaskState::active("NEXT")));
    let built = build_projection(
        vec![tasked],
        &holon_mcp::dense_projection::DocVocabularies::Uniform(
            holon_org_format::TaskKeywordVocabulary::default(),
        ),
    )
    .expect("projection builds");
    assert!(
        built.unfaithful.is_empty(),
        "a block shown as stored is not disclosed: {:?}\n{}",
        built.unfaithful,
        built.dense_text
    );
}

/// A headline's stars are its place: the store keeps no star count a plan
/// does not write, so stars other than the ones org writes at the row's
/// place are refused by the row's name.
#[test]
fn a_headline_with_other_stars_than_its_place_is_refused() {
    let blocks = [stored("a", "Row", &[], &[]), stored("b", "Next", &[], &[])];
    let (_, dense) = project(&blocks);
    for (from, to, name) in [
        ("* Row {#0}", "** Row {#0}", "{#0}"),
        ("* Next {#1}", "*** Next {#1}", "{#1}"),
        ("* Next {#1}", "* Next {#1}\n*** Child", "Child"),
    ] {
        let msg = refusal(&blocks, &edit(&dense, from, to));
        assert_names(&msg, &[name]);
        assert!(
            msg.contains("stars"),
            "{to:?} is refused for another reason: {msg}"
        );
    }
}

/// A body line with emphasis is an edit the store holds as written.
#[test]
fn a_marked_body_edit_plans() {
    let blocks = [
        stored("a", "Plan\na body line", &[], &[]),
        stored("b", "Other", &[], &[]),
    ];
    let (projection, dense) = project(&blocks);
    for line in ["x =====", "a *bold* b", "/x/ and ~y~"] {
        let text = edit(&dense, "a body line", line);
        if let Err(e) = plan(&projection, &text) {
            panic!("{line:?} is refused: {e:#}\n{text}");
        }
    }
}
