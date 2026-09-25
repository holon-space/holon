//! The `?` task keyword: an open question that an agent asks and a person
//! closes. It must parse as an active task state and survive
//! render → parse → render byte for byte, with and without a `#+TODO:` line.

use std::path::Path;

use holon_api::EntityUri;
use holon_api::StateCategory;
use holon_api::TaskState;
use holon_api::block::Block;
use holon_org_format::OrgBlockExt;
use holon_org_format::OrgRenderer;
use holon_org_format::TaskKeywordVocabulary;
use holon_org_format::converge_keyword_headed;
use holon_org_format::could_converge;
use holon_org_format::parse_org_file;
use holon_org_format::source_channel_commit;

const ROOT: &str = "/vault";
const FILE: &str = "/vault/questions.org";

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

fn assert_question_block_and_fixed_point(source: &str) {
    let (document, blocks) = parse(source);
    let question = block_titled(&blocks, "Delete fork branch?");
    let state = question
        .task_state()
        .expect("`?` headline must carry a task state");
    assert_eq!(state.keyword, "?");
    assert_eq!(state.category, StateCategory::Active);

    let option = block_titled(&blocks, "a · delete it");
    assert_eq!(option.task_state(), None, "an option is not a task");

    let first = render(&document, &blocks);
    assert_eq!(first, source, "render must reproduce the authored bytes");
    let (document, blocks) = parse(&first);
    assert_eq!(
        render(&document, &blocks),
        first,
        "render is not a fixed point"
    );
}

#[test]
fn undeclared_question_keyword_is_an_active_task_and_round_trips() {
    assert_question_block_and_fixed_point(
        "#+ID: questions\n\
         * DOING Land the fork PRs\n\
         :PROPERTIES:\n:ID: task\n:END:\n\
         ** ? Delete fork branch?\n\
         :PROPERTIES:\n:ID: q1\n:END:\n\
         *** a · delete it\n\
         :PROPERTIES:\n:ID: q1-a\n:END:\n",
    );
}

#[test]
fn declared_question_keyword_is_an_active_task_and_round_trips() {
    assert_question_block_and_fixed_point(
        "#+ID: questions\n\
         #+TODO: TODO ? | DONE\n\
         * TODO Land the fork PRs\n\
         :PROPERTIES:\n:ID: task\n:END:\n\
         ** ? Delete fork branch?\n\
         :PROPERTIES:\n:ID: q1\n:END:\n\
         *** a · delete it\n\
         :PROPERTIES:\n:ID: q1-a\n:END:\n",
    );
}

#[test]
fn question_property_keys_round_trip() {
    let source = "#+ID: questions\n\
                  * ? Delete fork branch?\n\
                  :PROPERTIES:\n:ID: q1\n\
                  :answer-read-by: claude-orch-0924 claude-orch-0925\n\
                  :asked-by: claude-orch-0924\n\
                  :decision: D214\n\
                  :kind: question\n:END:\n\
                  ** a · delete it\n\
                  :PROPERTIES:\n:ID: q1-a\n\
                  :option: a\n\
                  :q-role: option\n:END:\n";
    let (document, blocks) = parse(source);
    let question = block_titled(&blocks, "Delete fork branch?");
    for (key, value) in [
        ("kind", "question"),
        ("decision", "D214"),
        ("asked-by", "claude-orch-0924"),
        ("answer-read-by", "claude-orch-0924 claude-orch-0925"),
    ] {
        assert_eq!(
            question.get_property_str(key).as_deref(),
            Some(value),
            "{key}"
        );
    }
    let option = block_titled(&blocks, "a · delete it");
    assert_eq!(option.get_property_str("option").as_deref(), Some("a"));
    assert_eq!(option.get_property_str("q-role").as_deref(), Some("option"));

    let first = render(&document, &blocks);
    assert_eq!(first, source, "render must reproduce the authored bytes");
    let (document, blocks) = parse(&first);
    assert_eq!(
        render(&document, &blocks),
        first,
        "render is not a fixed point"
    );
}

#[test]
fn typed_question_keyword_is_a_convergence_candidate() {
    assert!(
        could_converge("? foo"),
        "`? foo` must reach the store's convergence"
    );
}

#[test]
fn a_question_mark_without_text_stays_prose() {
    let vocabulary = TaskKeywordVocabulary::default();
    for typed in ["?", "? ", "?\t ", "?  "] {
        assert!(
            !could_converge(typed),
            "{typed:?} must not reach the store's convergence"
        );
        assert!(
            !source_channel_commit("", typed),
            "typing {typed:?} into an empty block must stay on the content channel"
        );
        assert_eq!(
            converge_keyword_headed(typed, &vocabulary),
            None,
            "{typed:?} asks nothing, so it must not become a question"
        );
    }
    let promoted =
        converge_keyword_headed("? milk", &vocabulary).expect("`? milk` must become a question");
    assert_eq!(promoted.keyword, TaskState::active("?"));
    assert_eq!(promoted.stripped, "milk");
}

#[test]
fn a_bare_question_mark_headline_is_title_text() {
    for todo_line in ["", "#+TODO: TODO ? | DONE\n", "#+TODO: TODO | DONE ?\n"] {
        let source = format!(
            "#+ID: questions\n{todo_line}* ?\n:PROPERTIES:\n:ID: q-bare\n:END:\n\
             * ? pick a storage engine\n:PROPERTIES:\n:ID: q-real\n:END:\n"
        );
        let (document, blocks) = parse(&source);
        let bare = block_titled(&blocks, "?");
        assert_eq!(
            bare.task_state(),
            None,
            "`* ?` asks nothing, so it is title text (with {todo_line:?})"
        );
        let real = block_titled(&blocks, "pick a storage engine");
        assert_eq!(real.task_state().map(|s| s.keyword), Some("?".to_string()));

        let first = render(&document, &blocks);
        let (document, blocks) = parse(&first);
        assert_eq!(block_titled(&blocks, "?").task_state(), None);
        assert_eq!(
            render(&document, &blocks),
            first,
            "render is not a fixed point"
        );
    }
}
