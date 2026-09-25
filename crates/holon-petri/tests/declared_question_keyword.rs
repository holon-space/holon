//! Whether `?` is an open question comes from the document's own `#+TODO:`
//! vocabulary: declared after the `|`, it is a closed state like `DONE`.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;
use holon_org_format::parse_org_file;
use holon_petri::rank_tasks;

fn parse(source: &str) -> Vec<Block> {
    parse_org_file(
        Path::new("/vault/questions.org"),
        source,
        &EntityUri::no_parent(),
        Path::new("/vault"),
    )
    .expect("parse")
    .blocks
}

fn block_titled(blocks: &[Block], title: &str) -> Block {
    blocks
        .iter()
        .find(|b| b.title() == title)
        .unwrap_or_else(|| panic!("no block titled {title:?}"))
        .clone()
}

fn ranked_ids(source: &str) -> (Vec<String>, String) {
    let blocks = parse(source);
    let question = block_titled(&blocks, "pick a storage engine");
    let mut dependent = block_titled(&blocks, "migrate the store");
    dependent.set_property("depends_on", Value::String(question.id.to_string()));
    let result = rank_tasks(&[question, dependent.clone()]).expect("rank_tasks must succeed");
    (
        result.ranked.into_iter().map(|r| r.block_id).collect(),
        dependent.id.to_string(),
    )
}

#[test]
fn a_question_mark_declared_done_answers_its_dependents() {
    let (ranked, dependent) = ranked_ids(
        "#+TODO: TODO | DONE ?\n\
         * ? pick a storage engine\n\
         * TODO migrate the store\n",
    );
    assert_eq!(
        ranked,
        vec![dependent],
        "`?` is a DONE keyword in this document, so its dependent is free to run"
    );
}

#[test]
fn a_question_mark_declared_active_still_blocks_its_dependents() {
    let (ranked, _) = ranked_ids(
        "#+TODO: TODO ? | DONE\n\
         * ? pick a storage engine\n\
         * TODO migrate the store\n",
    );
    assert!(
        ranked.is_empty(),
        "an open `?` is not work and its dependent waits; ranked: {ranked:?}"
    );
}
