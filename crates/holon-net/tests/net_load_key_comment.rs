//! A trailing comment on a key line keeps the declaration's line in a
//! refusal.

use std::sync::Arc;

use holon_net::exec::Content;
use holon_net::exec::Reader;
use holon_net::exec::Subject;
use holon_net::load::Logic;
use holon_net::load::net_def;

const REPLAY_PATH: &str = "nets/work-runtime-replay.yaml";
const REPLAY: &str = include_str!("../nets/work-runtime-replay.yaml");

fn inert(_: &str) -> Logic {
    Logic::pure(
        Arc::new(|_: &mut Reader<'_>, _: &Subject| None),
        Arc::new(|_: &Subject, _: &Content| Ok(vec![])),
    )
}

fn edit(text: &str, from: &str, to: &str) -> String {
    let out = text.replace(from, to);
    assert_ne!(out, text, "the edit {from:?} must hit");
    out
}

/// `StartJob` inhibits an undeclared place; the refusal must name the line
/// whose text declares `StartJob`.
fn assert_names_the_start_job_line(text: &str) {
    let text = edit(text, "inhibits:\n    - Cpu\n", "inhibits:\n    - Cpuu\n");
    let err = match net_def(REPLAY_PATH, &text, inert) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("an arc onto an undeclared place must be refused"),
    };
    let want = text
        .lines()
        .position(|l| l.trim_start().starts_with("StartJob:"))
        .expect("the transition is declared")
        + 1;
    assert!(
        err.starts_with(&format!("{REPLAY_PATH}:{want}:")),
        "the refusal must name the declaration's line {want}: {err}"
    );
}

#[test]
fn a_trailing_comment_on_the_declaration_keeps_its_line() {
    assert_names_the_start_job_line(&edit(
        REPLAY,
        "\n  StartJob:\n",
        "\n  StartJob: # starts a job\n",
    ));
}

#[test]
fn a_trailing_comment_on_the_section_key_keeps_the_line() {
    assert_names_the_start_job_line(&edit(
        REPLAY,
        "transitions:\n",
        "transitions: # every transition\n",
    ));
}

#[test]
fn the_authored_file_names_the_line() {
    assert_names_the_start_job_line(REPLAY);
}
