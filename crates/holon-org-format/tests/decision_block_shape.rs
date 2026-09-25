//! A decision is a block shape: the headline tag `decision`, the task keyword
//! `?` / DONE / CANCELLED for its lifecycle, option and answer children told
//! apart by one drawer key each, and the ruling keys on the decision block.
//! Every key and value of that shape must survive render → parse → render
//! byte for byte, in every lifecycle state.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::block::Block;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;

const ROOT: &str = "/vault";
const FILE: &str = "/vault/decisions.org";
const TITLE: &str = "D214 Delete fork branch holon/deleted-container-purge?";

fn parse(source: &str) -> (Block, Vec<Block>) {
    let parsed = parse_org_file(
        Path::new(FILE),
        source,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("parse");
    (parsed.document, parsed.blocks)
}

fn render(document: &Block, blocks: &[Block]) -> String {
    OrgRenderer::render_document(document, blocks, Path::new(FILE), &document.id)
}

fn block_titled<'a>(blocks: &'a [Block], title: &str) -> &'a Block {
    blocks
        .iter()
        .find(|b| b.title() == title)
        .unwrap_or_else(|| {
            let titles: Vec<String> = blocks.iter().map(Block::title).collect();
            panic!("no block titled {title:?}; titles: {titles:?}")
        })
}

fn assert_properties(block: &Block, expected: &[(&str, &str)]) {
    for (key, value) in expected {
        assert_eq!(
            block.get_property_str(key).as_deref(),
            Some(*value),
            "{key} on {:?}",
            block.title()
        );
    }
}

fn assert_byte_stable(source: &str) {
    let (document, blocks) = parse(source);
    let first = render(&document, &blocks);
    assert_eq!(first, source, "render must reproduce the authored bytes");
    let (document, blocks) = parse(&first);
    assert_eq!(
        render(&document, &blocks),
        first,
        "render is not a fixed point"
    );
}

fn decision_file(keyword: &str, ruling: &str) -> String {
    format!(
        "#+ID: decisions\n\
         * Sharing\n\
         :PROPERTIES:\n:ID: topic\n\
         :decision-prefix: sharing\n:END:\n\
         ** {keyword} {TITLE} :decision:\n\
         :PROPERTIES:\n:ID: sharing-12\n\
         :choose: 1..3\n\
         :recommend: a\n\
         :asked-by: agent:claude-orch-0924\n\
         :read-by: agent:claude-orch-0924 agent:claude-orch-0925\n\
         :supersedes: sharing-7\n\
         {ruling}:END:\n\
         Its commit is reachable through d70-rebase; nothing else names it.\n\
         *** Delete it\n\
         :PROPERTIES:\n:ID: sharing-12-a\n\
         :option: a\n:END:\n\
         **** pro: one less branch to keep in sync\n\
         :PROPERTIES:\n:ID: sharing-12-a-pro\n:END:\n\
         *** Rename our bookmark instead of \"holon\"\n\
         :PROPERTIES:\n:ID: sharing-12-b\n\
         :option: b\n:END:\n\
         *** Keep both\n\
         :PROPERTIES:\n:ID: sharing-12-c\n\
         :option: c\n:END:\n\
         *** Suggest a (0.82)\n\
         :PROPERTIES:\n:ID: sharing-12-s1\n\
         :answerer: model:jev-1\n\
         :p: a=0.82 b=0.18\n\
         :answered: 2026-09-25T10:04:00Z\n:END:\n"
    )
}

/// Keys set on a parsed block are appended in sorted order, so a ruling that
/// a write adds reads `chosen`, `decided`, `decider`.
const RULING: &str = ":chosen: a c\n\
                      :decided: 2026-09-25T10:12:31Z\n\
                      :decider: person:martin\n";

fn assert_decision_shape(source: &str, keyword: &str) {
    let (_, blocks) = parse(source);

    let topic = block_titled(&blocks, "Sharing");
    assert_properties(topic, &[("decision-prefix", "sharing")]);

    let decision = block_titled(&blocks, TITLE);
    assert!(
        decision.tags().contains("decision"),
        "tags: {:?}",
        decision.tags()
    );
    assert_eq!(
        decision.task_state().map(|s| s.keyword),
        Some(keyword.to_string())
    );
    assert_properties(
        decision,
        &[
            ("choose", "1..3"),
            ("recommend", "a"),
            ("asked-by", "agent:claude-orch-0924"),
            ("read-by", "agent:claude-orch-0924 agent:claude-orch-0925"),
            ("supersedes", "sharing-7"),
        ],
    );

    for (title, key) in [
        ("Delete it", "a"),
        ("Rename our bookmark instead of \"holon\"", "b"),
        ("Keep both", "c"),
    ] {
        let option = block_titled(&blocks, title);
        assert_properties(option, &[("option", key)]);
        assert_eq!(option.task_state(), None, "an option is not a task");
        assert!(option.tags().is_empty(), "an option carries no tag");
    }

    let answer = block_titled(&blocks, "Suggest a (0.82)");
    assert_properties(
        answer,
        &[
            ("answerer", "model:jev-1"),
            ("p", "a=0.82 b=0.18"),
            ("answered", "2026-09-25T10:04:00Z"),
        ],
    );

    assert_byte_stable(source);
}

#[test]
fn open_decision_round_trips() {
    assert_decision_shape(&decision_file("?", ""), "?");
}

#[test]
fn decided_decision_round_trips_with_its_ruling() {
    let source = decision_file("DONE", RULING);
    assert_decision_shape(&source, "DONE");
    let (_, blocks) = parse(&source);
    assert_properties(
        block_titled(&blocks, TITLE),
        &[
            ("chosen", "a c"),
            ("decider", "person:martin"),
            ("decided", "2026-09-25T10:12:31Z"),
        ],
    );
}

#[test]
fn withdrawn_decision_round_trips() {
    assert_decision_shape(&decision_file("CANCELLED", ""), "CANCELLED");
}

#[test]
fn tag_survives_a_keyword_change() {
    for (from, to, ruling) in [("?", "DONE", RULING), ("?", "CANCELLED", "")] {
        let (document, mut blocks) = parse(&decision_file(from, ""));
        let decision = blocks
            .iter_mut()
            .find(|b| b.title() == TITLE)
            .expect("decision block");
        let target = parse(&decision_file(to, ruling)).1;
        let target = block_titled(&target, TITLE);
        decision.set_task_state(target.task_state());
        for key in ["chosen", "decider", "decided"] {
            if let Some(value) = target.get_property_str(key) {
                decision.set_property(key, holon_api::Value::String(value));
            }
        }
        assert_eq!(
            render(&document, &blocks),
            decision_file(to, ruling),
            "{from} -> {to}"
        );
    }
}

/// Emacs aligns tags with padding; the renderer writes one space. The change
/// is whitespace only and the first write is already the fixed point.
#[test]
fn aligned_tag_is_normalized_to_one_space() {
    let aligned = decision_file("?", "").replace(" :decision:", "      :decision:");
    let (document, blocks) = parse(&aligned);
    let first = render(&document, &blocks);
    assert_eq!(first, decision_file("?", ""));
    assert_byte_stable(&first);
}

/// Tags are a set; several tags are written in sorted order.
#[test]
fn several_tags_are_written_sorted() {
    let authored = decision_file("?", "").replace(":decision:", ":human-only:decision:");
    let (document, blocks) = parse(&authored);
    let first = render(&document, &blocks);
    assert_eq!(
        first,
        decision_file("?", "").replace(":decision:", ":decision:human-only:")
    );
    assert_byte_stable(&first);
}
