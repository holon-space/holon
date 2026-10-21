//! Probes of the loader's refusals, written through the public entry points
//! the crate's own tests use.

use std::sync::Arc;

use holon_net::exec::Content;
use holon_net::exec::Reader;
use holon_net::exec::Subject;
use holon_net::load::Logic;
use holon_net::load::net_def;

const BOARD_PATH: &str = "nets/work-runtime.yaml";
const BOARD: &str = include_str!("../nets/work-runtime.yaml");

/// A guard that binds nothing and a body that writes nothing.
fn inert(_: &str) -> Logic {
    Logic::pure(
        Arc::new(|_: &mut Reader<'_>, _: &Subject| None),
        Arc::new(|_: &Subject, _: &Content| Ok(vec![])),
    )
}

/// Every 1-based line the declaration `key` is on.
fn lines_of(text: &str, key: &str) -> Vec<usize> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| l.trim_start() == format!("{key}:"))
        .map(|(i, _)| i + 1)
        .collect()
}

/// `nets/work-runtime.yaml` declares a place `Stalled` AND a transition
/// `Stalled`. A refusal about the transition must name the transition's
/// declaration, not the place of the same name.
#[test]
fn a_refusal_names_its_own_declaration_when_a_place_shares_the_name() {
    let text = BOARD.replace("    capacity: fifo\n    pool: 4\n", "    capacity: fifo\n");
    assert!(!text.contains("pool:"), "the pool must be gone");
    let broken = text.replace(
        "  Stalled:\n    trigger: auto\n    priority: 1\n    reads:\n    - Agent\n",
        "  Stalled:\n    trigger: auto\n    priority: 1\n    reads:\n    - Agentt\n",
    );
    assert_ne!(broken, text, "the edit must hit the transition");
    let err = match net_def(BOARD_PATH, &broken, inert) {
        Err(e) => e.to_string(),
        Ok(_) => panic!("an arc onto an undeclared place must be refused"),
    };
    let declarations = lines_of(&broken, "Stalled");
    assert_eq!(declarations.len(), 2, "place and transition share the name");
    let transition_line = declarations[1];
    assert!(
        err.starts_with(&format!("{BOARD_PATH}:{transition_line}:")),
        "the refusal must name the transition's line {transition_line}: {err}"
    );
}
