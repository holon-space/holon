//! The loader: one net file becomes the executor's `NetDef`, and every
//! refusal names the file, the line of the declaration it refused and the
//! field.
//!
//! The arcs, the capacities and the ports come from the file; the guard and
//! the body of a firing are Rust the file cannot hold, so the caller hands
//! them in.

use std::sync::Arc;

use holon_net::exec::Content;
use holon_net::exec::Executor;
use holon_net::exec::LoadError;
use holon_net::exec::Place;
use holon_net::exec::Reader;
use holon_net::exec::Subject;
use holon_net::file::NetFile;
use holon_net::load::Logic;
use holon_net::load::net_def;

const REPLAY_PATH: &str = "nets/work-runtime-replay.yaml";
const REPLAY: &str = include_str!("../nets/work-runtime-replay.yaml");

/// A guard that binds nothing and a body that writes nothing: the loader
/// builds the net's shape, which these tests read, and never calls them.
fn inert(_: &str) -> Logic {
    Logic::pure(
        Arc::new(|_: &mut Reader<'_>, _: &Subject| None),
        Arc::new(|_: &Subject, _: &Content| Ok(vec![])),
    )
}

/// The refusal of `text`, as a reader sees it. Neither `NetDef` nor
/// `Executor` is `Debug`, so the tests unwrap by hand.
fn refusal(path: &str, text: &str) -> String {
    match net_def(path, text, inert) {
        Err(err) => err.to_string(),
        Ok(_) => panic!("the net is malformed, so the loader must refuse it"),
    }
}

/// `base` with `from` replaced by `to`, asserting the edit hit something.
fn edited(base: &str, from: &str, to: &str) -> String {
    assert!(base.contains(from), "`{from}` is not in the base net");
    base.replace(from, to)
}

/// The 1-based line the declaration `key` is on. It must be the only line
/// that declares the name, so the expected line never comes from the section
/// search the loader itself uses.
fn line_of(text: &str, key: &str) -> usize {
    let lines: Vec<usize> = text
        .lines()
        .enumerate()
        .filter(|(_, l)| l.trim_start() == format!("{key}:"))
        .map(|(index, _)| index + 1)
        .collect();
    assert_eq!(lines.len(), 1, "`{key}` is declared once in this net");
    lines[0]
}

#[test]
fn the_replay_net_is_canonical() {
    let net = NetFile::parse(REPLAY).expect("the replay net parses");
    assert_eq!(
        net.to_yaml(),
        REPLAY,
        "the writer must reproduce the replay net byte for byte"
    );
}

#[test]
fn the_replay_net_loads_into_the_executor() {
    let Ok(net) = net_def(REPLAY_PATH, REPLAY, inert) else {
        panic!("the replay net loads")
    };
    assert!(
        Executor::new(net).is_ok(),
        "the loaded net must pass the executor's checks"
    );
}

#[test]
fn an_arc_onto_an_undeclared_place_names_the_file_the_line_and_the_field() {
    let text = edited(
        REPLAY,
        "    consumes:\n    - Job\n    - Cpu\n",
        "    consumes:\n    - Job\n    - Cpuu\n",
    );
    let err = refusal(REPLAY_PATH, &text);
    assert_eq!(
        err,
        format!(
            "{REPLAY_PATH}:{}: transition `JobEnded`: `consumes` names the place `Cpuu`, which the file does not declare",
            line_of(&text, "JobEnded")
        )
    );
}

#[test]
fn a_malformed_file_names_the_file_and_the_parser_position() {
    let text = edited(
        REPLAY,
        "    trigger: auto\n    priority: 0\n    consumes:\n    - TurnEnded",
        "    trigger: sometimes\n    priority: 0\n    consumes:\n    - TurnEnded",
    );
    let err = refusal(REPLAY_PATH, &text);
    assert!(
        err.starts_with(&format!("{REPLAY_PATH}: "))
            && err.contains("trigger")
            && err.contains("line 27"),
        "the refusal must name the file and the position: {err}"
    );
}

#[test]
fn the_executor_checks_a_loaded_net_too() {
    let text = edited(
        REPLAY,
        "  Woken:\n    capacity: single\n",
        "  Woken:\n    capacity: single\n  StartJob.Running:\n    capacity: single\n",
    );
    let net = match net_def(REPLAY_PATH, &text, inert) {
        Ok(net) => net,
        Err(e) => panic!("the net file itself is well formed: {e}"),
    };
    let Err(refused) = Executor::new(net) else {
        panic!("the executor owns that place")
    };
    assert_eq!(
        refused,
        LoadError::ReservedPlace(Place::new("StartJob.Running"))
    );
}

#[test]
fn a_declared_pool_loads_with_all_its_tokens_free() {
    let text = "\
name: slots
places:
  Waiting:
    capacity: fifo
  Cpu:
    capacity: fifo
    pool: 4
  Held:
    capacity: single
transitions:
  Take:
    trigger: auto
    priority: 0
    consumes:
    - Waiting
    - Cpu
    produces:
    - Held
";
    let net = net_def("slots.yaml", text, inert).expect("the net loads");
    let ex = Executor::new(net).expect("the net checks");
    assert_eq!(ex.tokens().free("Cpu"), 4);
}

/// A net whose one transition runs a shell effect, with the keyed receiver
/// that lets the effect run again and the count bound that stops it.
const WITH_EFFECT: &str = "\
name: effectful
places:
  Waiting:
    capacity: fifo
  Stalled:
    capacity: single
transitions:
  Take:
    trigger: auto
    priority: 0
    consumes:
    - Waiting
    on_failure: Stalled
    on_timeout: Stalled
    effect: run
    receiver: keyed
    timers:
      run: 5m
      count_bound: 3
      backoff: 1m
effects:
  run:
    kind: shell
    command:
      argv:
      - /bin/true
      working_dir: .
      env_allow: []
      timeout: 1s
    log_path: target/net/{attempt}.log
    exit_codes:
      done:
      - 0
      failed:
      - 1
      other: outcome_unknown
";

/// The firing of a transition the file declares an effect for is that effect,
/// not Rust, and its retry comes from the `timers` block: a keyed receiver
/// whose count bound the loader dropped is refused by the executor with
/// [`LoadError::KeyedWithoutCountBound`].
#[test]
fn a_declared_effect_becomes_the_firing_with_its_retry() {
    let logic = |_: &str| Logic::effect(Arc::new(|_: &mut Reader<'_>, _: &Subject| None));
    let net = match net_def("effectful.yaml", WITH_EFFECT, logic) {
        Ok(net) => net,
        Err(e) => panic!("the effectful net loads: {e}"),
    };
    assert!(
        Executor::new(net).is_ok(),
        "the keyed receiver must carry the file's count bound"
    );
}
