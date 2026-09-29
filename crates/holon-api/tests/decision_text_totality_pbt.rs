//! A text-only edit never turns a decision the block adapter accepts into one
//! it refuses.
//!
//! @pbt oracle metamorphic — parse(subtree) is Ok before the edit, so it is
//!   Ok after replacing any blocks' text with any text
//! @pbt covers decision-text-totality — the premise the shape gate relies on
//!   to never judge a text write (`DecisionShape::judges_text` is `false`)
//! @pbt slips-if-removed a text write in an answer's body or a title makes the
//!   decision unreadable, and the shape gate lets it land unjudged
//!
//! The decision's task keyword is a property, not text; a source-line write
//! that changes it is judged (Model.md invariant 17).

use std::collections::HashMap;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::decision_block;
use proptest::prelude::*;

fn block(id: &str, parent: &EntityUri, content: &str, props: &[(&str, &str)]) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), parent.clone(), content);
    b.properties = props
        .iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
        .collect::<HashMap<_, _>>();
    b
}

/// The status a fixture decision is in.
#[derive(Debug, Clone, Copy)]
enum Status {
    Open,
    Decided,
    Withdrawn,
}

/// A legal decision: `options` options (2..=4), multi-select when `multi`,
/// optionally a recommendation and a superseded decision, and answers of
/// every body kind, each with or without a rationale.
fn subtree(
    status: Status,
    options: usize,
    multi: bool,
    rationales: [bool; 3],
) -> (Block, Vec<Block>) {
    let page = EntityUri::block("page");
    let keys = ["a", "b", "c", "d"];
    let choose = if multi { "1..2" } else { "1" };
    let mut props: Vec<(&str, &str)> = vec![("choose", choose), ("recommend", "a")];
    props.push(("supersedes", "d-old"));
    match status {
        Status::Open => props.push(("task_state", "?")),
        Status::Decided => props.extend([
            ("task_state", "DONE"),
            ("chosen", if multi { "a b" } else { "a" }),
            ("decider", "person:martin"),
            ("decided", "2026-09-29T10:00:00Z"),
            ("note", "because"),
        ]),
        Status::Withdrawn => props.extend([
            ("task_state", "CANCELLED"),
            ("withdrawer", "person:martin"),
            ("withdrawn", "2026-09-29T10:00:00Z"),
        ]),
    }
    let mut root = block("d", &page, "Which store?", &props);
    root.tags.insert(decision_block::DECISION_TAG.to_string());
    let id = root.id.clone();
    let mut children: Vec<Block> = keys[..options]
        .iter()
        .map(|k| {
            block(
                &format!("d-{k}"),
                &id,
                &format!("Option {k}"),
                &[("option", k)],
            )
        })
        .collect();
    let body = |with: bool, title: &str| {
        if with {
            format!("{title}\nit merges well")
        } else {
            title.to_string()
        }
    };
    let pick = if multi { "a b" } else { "a" };
    children.push(block(
        "d-s1",
        &id,
        &body(rationales[0], "Answer"),
        &[
            ("answerer", "model:jev-1"),
            ("pick", pick),
            ("answered", "2026-09-29T09:00:00Z"),
        ],
    ));
    if !multi {
        children.push(block(
            "d-s2",
            &id,
            &body(rationales[1], "Answer"),
            &[
                ("answerer", "agent:claude"),
                ("p", "a=0.7 b=0.2"),
                ("answered", "2026-09-29T09:05:00Z"),
            ],
        ));
    }
    children.push(block(
        "d-s3",
        &id,
        &body(rationales[2], "Answer"),
        &[
            ("answerer", "person:martin"),
            ("marginal", "a=0.9 b=0.4"),
            ("answered", "2026-09-29T09:10:00Z"),
        ],
    ));
    children.push(block("d-n", &id, "a note", &[]));
    (root, children)
}

fn status() -> impl Strategy<Value = Status> {
    prop_oneof![
        Just(Status::Open),
        Just(Status::Decided),
        Just(Status::Withdrawn)
    ]
}

/// Any text, including what org treats specially: keywords, stars, drawer
/// lines, links, digits, quotes, backslashes and line breaks.
fn any_text() -> impl Strategy<Value = String> {
    prop_oneof![
        "[ \\t\\na-zA-Z0-9?:*#\\[\\]\"'\\\\=.-]{0,60}",
        Just(String::new()),
        Just("DONE ".to_string()),
        Just("? ".to_string()),
        Just(":PROPERTIES:\n:option: z\n:END:".to_string()),
        Just("* heading\n** child".to_string()),
        Just("Answer\n   \n\t".to_string()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn a_text_edit_keeps_a_readable_decision_readable(
        status in status(),
        options in 2usize..=4,
        multi in any::<bool>(),
        rationales in any::<[bool; 3]>(),
        edits in prop::collection::vec((0usize..10, any_text()), 1..4),
    ) {
        let (mut root, mut children) = subtree(status, options, multi, rationales);
        decision_block::parse(&root, &children).expect("every fixture is a legal decision");
        for (target, text) in &edits {
            match target % (children.len() + 1) {
                0 => root.content = text.clone(),
                i => children[i - 1].content = text.clone(),
            }
        }
        let after = decision_block::parse(&root, &children);
        prop_assert!(after.is_ok(), "edits {edits:?} on {status:?} broke the parse: {after:?}");
    }
}
