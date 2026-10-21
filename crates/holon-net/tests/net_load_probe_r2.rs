//! Verifier probes of the loader's line resolution on legal, non-canonical
//! files.

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

/// Break the `JobEnded` transition's `consumes` arc onto an undeclared place.
fn broken(text: &str) -> String {
    let out = text.replace("- Cpu\n    produces:", "- Cpuu\n    produces:");
    assert_ne!(out, text, "the edit must hit an arc");
    out
}

fn refusal(path: &str, text: &str) -> String {
    match net_def(path, text, inert) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("an arc onto an undeclared place must be refused"),
    }
}

/// Every line below the top-level keys gets two more spaces: still one legal
/// YAML file with the same content, only not the writer's canonical indent.
fn reindented(text: &str) -> String {
    text.lines()
        .map(|l| {
            if l.starts_with("  ") {
                format!("  {l}\n")
            } else {
                format!("{l}\n")
            }
        })
        .collect()
}

#[test]
fn a_four_space_indented_file_still_gets_the_line_of_its_declaration() {
    let text = reindented(&broken(REPLAY));
    let err = refusal(REPLAY_PATH, &text);
    assert!(
        err.starts_with(&format!("{REPLAY_PATH}:"))
            && err[REPLAY_PATH.len() + 1..].starts_with(|c: char| c.is_ascii_digit()),
        "the refusal must name a line: {err}"
    );
}

#[test]
fn a_comment_in_a_section_does_not_cost_the_next_declaration_its_line() {
    let text = REPLAY.replace("transitions:\n", "transitions:\n# the agent arrives\n");
    assert_ne!(text, REPLAY, "the comment must be inserted");
    let err = refusal(REPLAY_PATH, &broken(&text));
    assert!(
        err.starts_with(&format!("{REPLAY_PATH}:"))
            && err[REPLAY_PATH.len() + 1..].starts_with(|c: char| c.is_ascii_digit()),
        "the refusal must name a line: {err}"
    );
}

#[test]
fn a_place_refusal_names_the_place_when_the_transitions_come_first() {
    let at = REPLAY
        .find("transitions:\n")
        .expect("the file has transitions");
    let head = &REPLAY[..at];
    let tail = &REPLAY[at..];
    let name_end = head.find('\n').expect("the first line is the name") + 1;
    let reordered = format!("{}{}{}", &head[..name_end], tail, &head[name_end..]);
    let text = reordered.replace(
        "  Stalled:\n    capacity: single\n",
        "  Stalled:\n    capacity: fifo\n    pool: 0\n",
    );
    assert_ne!(text, reordered, "the edit must hit the place");
    let err = match net_def(REPLAY_PATH, &text, inert) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("an empty pool must be refused"),
    };
    let want = text
        .lines()
        .position(|l| l == "  Stalled:")
        .expect("the place is declared")
        + 1;
    assert!(
        err.starts_with(&format!("{REPLAY_PATH}:{want}:")),
        "the refusal must name the place's line {want}: {err}"
    );
}
