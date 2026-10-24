//! The standalone net file format (#369): the example file round-trips
//! byte-stable, and every load error names the field it refuses.
//!
//! The format is a contract, so the assertions read the typed shape the
//! loader (#370), the shell effect (#371) and the probe host (#373) will
//! consume — not only that parsing succeeded.

use std::time::Duration;

use holon_net::file::EffectSpec;
use holon_net::file::NetFile;
use holon_net::file::Outcome;
use holon_net::file::PlaceCapacity;
use holon_net::file::Receiver;

/// The work-runtime net of #363, in the canonical form the writer emits.
const EXAMPLE: &str = include_str!("../nets/work-runtime.yaml");

/// A net small enough that one replaced line makes exactly one error.
const MINIMAL: &str = "\
name: minimal
places:
  Waiting:
    capacity: fifo
transitions:
  Take:
    trigger: auto
    priority: 0
    consumes:
    - Waiting
";

/// `MINIMAL` plus one shell effect, for the descriptor's own refusals.
const WITH_EFFECT: &str = "\
name: minimal
places:
  Waiting:
    capacity: fifo
transitions:
  Take:
    trigger: auto
    priority: 0
    consumes:
    - Waiting
    effect: run
    receiver: none
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

fn refusal(text: &str) -> String {
    NetFile::parse(text)
        .expect_err("the net is malformed, so parsing must refuse it")
        .to_string()
}

/// `base` with `from` replaced by `to`, asserting the edit hit something.
fn edited(base: &str, from: &str, to: &str) -> String {
    assert!(base.contains(from), "`{from}` is not in the base net");
    base.replace(from, to)
}

#[test]
fn the_example_net_is_written_back_byte_stable() {
    let net = NetFile::parse(EXAMPLE).expect("the example net parses");
    assert_eq!(
        net.to_yaml(),
        EXAMPLE,
        "the writer must reproduce the canonical file byte for byte"
    );
}

#[test]
fn the_minimal_nets_of_this_test_are_canonical() {
    for text in [MINIMAL, WITH_EFFECT] {
        let net = NetFile::parse(text).expect("the net parses");
        assert_eq!(
            net.to_yaml(),
            text,
            "a non-canonical fixture would make the refusal tests edit text the writer never emits"
        );
    }
}

#[test]
fn the_example_net_carries_the_whole_schema() {
    let net = NetFile::parse(EXAMPLE).expect("the example net parses");

    assert_eq!(net.name, "work-runtime");

    let cpu = &net.places["Cpu"];
    assert_eq!(cpu.capacity, PlaceCapacity::Fifo);
    assert_eq!(cpu.pool, Some(4), "Cpu is the pool of build slots");
    assert_eq!(
        net.places["Agent"].capacity,
        PlaceCapacity::Register,
        "a probe re-derives the agent fact, so its put replaces the token"
    );

    let start = &net.transitions["StartJob"];
    assert_eq!(start.consumes, ["Waiting", "Cpu"]);
    assert_eq!(start.reads, ["Budget"]);
    assert_eq!(start.inhibits, ["Job"]);
    assert_eq!(start.produces, ["Job"]);
    assert_eq!(start.effect.as_deref(), Some("run_job"));
    assert_eq!(
        start.receiver,
        Some(Receiver::None),
        "a job run has no keyed receiver, so an unknown outcome parks"
    );
    assert_eq!(
        start.on_failure.as_deref(),
        Some("Stalled"),
        "the failure port is where a settled-failed run puts its token"
    );
    assert_eq!(start.on_timeout.as_deref(), Some("Stalled"));
    let timers = start.timers.as_ref().expect("StartJob declares timers");
    assert_eq!(
        timers.run.map(|d| d.duration()),
        Some(Duration::from_secs(2 * 3600))
    );
    assert_eq!(
        timers.abandon.map(|d| d.duration()),
        Some(Duration::from_secs(3 * 3600))
    );
    assert_eq!(timers.count_bound, Some(2));
    assert_eq!(
        timers.backoff.map(|d| d.duration()),
        Some(Duration::from_secs(5 * 60)),
        "a count bound without a backoff could not build the executor's Retry"
    );
    assert_eq!(
        net.transitions["Handoff"].receiver,
        Some(Receiver::Keyed),
        "the handoff writer dedups by occurrence id, so an unknown outcome retries"
    );

    let EffectSpec::Shell(shell) = &net.effects["run_job"];
    assert_eq!(shell.command.argv[0], "bash");
    assert_eq!(shell.command.env_allow, ["HOME", "PATH"]);
    assert_eq!(
        shell.command.timeout.duration(),
        Duration::from_secs(2 * 3600)
    );
    assert_eq!(shell.exit_codes.done, [0]);
    assert_eq!(shell.exit_codes.failed, [1, 2]);
    assert_eq!(
        shell.exit_codes.other,
        Outcome::OutcomeUnknown,
        "an unmapped exit code leaves the outcome unknown, never Done"
    );

    let probe = &net.probes["registry_agents"];
    assert_eq!(probe.interval.duration(), Duration::from_secs(60));
    assert_eq!(probe.facts_into, "Agent");
    assert_eq!(probe.subject_field, "agentId");
}

#[test]
fn an_unknown_place_key_names_the_key() {
    let text = edited(
        MINIMAL,
        "    capacity: fifo",
        "    capacity: fifo\n    pool_size: 4",
    );
    let message = refusal(&text);
    assert!(
        message.contains("pool_size"),
        "the refusal must name the unknown key: {message}"
    );
}

#[test]
fn a_command_without_argv_names_the_missing_field() {
    let text = edited(WITH_EFFECT, "      argv:\n      - /bin/true\n", "");
    let message = refusal(&text);
    assert!(
        message.contains("argv"),
        "the refusal must name the missing field: {message}"
    );
}

#[test]
fn an_empty_argv_names_the_field() {
    let text = edited(
        WITH_EFFECT,
        "      argv:\n      - /bin/true",
        "      argv: []",
    );
    let message = refusal(&text);
    assert!(
        message.contains("run") && message.contains("argv"),
        "the refusal must name the effect and the field: {message}"
    );
}

#[test]
fn an_unknown_key_inside_an_effect_names_the_key() {
    let text = edited(
        WITH_EFFECT,
        "    log_path:",
        "    nice_level: 10\n    log_path:",
    );
    let message = refusal(&text);
    assert!(
        message.contains("nice_level"),
        "a tagged descriptor must refuse an unknown key too: {message}"
    );
}

#[test]
fn an_unknown_effect_kind_names_the_kind() {
    let message = refusal(&edited(WITH_EFFECT, "kind: shell", "kind: http"));
    assert!(
        message.contains("http") && message.contains("shell"),
        "the refusal must name the kind it got and the kinds it knows: {message}"
    );
}

#[test]
fn a_bad_capacity_names_the_field_and_the_word() {
    let message = refusal(&edited(MINIMAL, "capacity: fifo", "capacity: unbounded"));
    assert!(
        message.contains("capacity") && message.contains("unbounded"),
        "the refusal must name the field and the word it refused: {message}"
    );
}

#[test]
fn a_pool_outside_a_fifo_place_is_refused() {
    let text = edited(
        MINIMAL,
        "    capacity: fifo",
        "    capacity: register\n    pool: 4",
    );
    let message = refusal(&text);
    assert!(
        message.contains("Waiting") && message.contains("pool"),
        "the refusal must name the place and the field: {message}"
    );
}

#[test]
fn a_pool_of_zero_tokens_is_refused() {
    let text = edited(
        MINIMAL,
        "    capacity: fifo",
        "    capacity: fifo\n    pool: 0",
    );
    let message = refusal(&text);
    assert!(
        message.contains("Waiting") && message.contains("pool"),
        "the refusal must name the place and the field: {message}"
    );
}

#[test]
fn an_arc_onto_an_undeclared_place_is_refused() {
    let message = refusal(&edited(MINIMAL, "    - Waiting", "    - Ready"));
    assert!(
        message.contains("Take") && message.contains("Ready") && message.contains("consumes"),
        "the refusal must name the transition, the field and the place: {message}"
    );
}

#[test]
fn a_transition_without_an_input_arc_is_refused() {
    let message = refusal(&edited(
        MINIMAL,
        "    consumes:\n    - Waiting",
        "    produces:\n    - Waiting",
    ));
    assert!(
        message.contains("Take"),
        "the refusal must name the transition: {message}"
    );
}

#[test]
fn an_effect_that_no_descriptor_declares_is_refused() {
    let message = refusal(&edited(
        MINIMAL,
        "    priority: 0",
        "    priority: 0\n    effect: run_job",
    ));
    assert!(
        message.contains("Take") && message.contains("run_job"),
        "the refusal must name the transition and the effect: {message}"
    );
}

#[test]
fn a_timers_block_without_a_timer_is_refused() {
    let message = refusal(&edited(
        MINIMAL,
        "    priority: 0",
        "    priority: 0\n    timers: {}",
    ));
    assert!(
        message.contains("Take") && message.contains("timers"),
        "the refusal must name the transition and the field: {message}"
    );
}

#[test]
fn a_duplicate_place_key_is_refused() {
    let message = refusal(&edited(
        MINIMAL,
        "  Waiting:\n    capacity: fifo",
        "  Waiting:\n    capacity: fifo\n  Waiting:\n    capacity: single",
    ));
    assert!(
        message.contains("Waiting"),
        "the refusal must name the duplicated key: {message}"
    );
}

#[test]
fn an_exit_code_in_two_outcomes_is_refused() {
    let message = refusal(&edited(
        WITH_EFFECT,
        "      failed:\n      - 1",
        "      failed:\n      - 0",
    ));
    assert!(
        message.contains("run") && message.contains('0'),
        "the refusal must name the effect and the code: {message}"
    );
}

#[test]
fn an_effect_without_a_receiver_kind_is_refused() {
    let message = refusal(&edited(WITH_EFFECT, "\n    receiver: none", ""));
    assert!(
        message.contains("Take") && message.contains("receiver"),
        "the refusal must name the transition and the field: {message}"
    );
}

#[test]
fn a_receiver_kind_without_an_effect_is_refused() {
    let message = refusal(&edited(
        MINIMAL,
        "    priority: 0",
        "    priority: 0\n    receiver: keyed",
    ));
    assert!(
        message.contains("Take") && message.contains("receiver"),
        "a pure transition has no outcome to resolve: {message}"
    );
}

#[test]
fn a_keyed_receiver_without_a_count_bound_is_refused() {
    let message = refusal(&edited(WITH_EFFECT, "receiver: none", "receiver: keyed"));
    assert!(
        message.contains("Take") && message.contains("count_bound"),
        "a keyed receiver retries, so it needs the bound: {message}"
    );
}

#[test]
fn a_count_bound_without_a_backoff_is_refused() {
    let message = refusal(&edited(
        MINIMAL,
        "    priority: 0",
        "    priority: 0\n    timers:\n      count_bound: 3",
    ));
    assert!(
        message.contains("Take") && message.contains("backoff"),
        "the refusal must name the transition and the missing field: {message}"
    );
}

#[test]
fn a_failure_port_onto_an_undeclared_place_is_refused() {
    let message = refusal(&edited(
        MINIMAL,
        "    priority: 0",
        "    priority: 0\n    on_failure: Stalled",
    ));
    assert!(
        message.contains("Take") && message.contains("Stalled") && message.contains("on_failure"),
        "the refusal must name the transition, the field and the place: {message}"
    );
}

#[test]
fn an_unknown_log_path_placeholder_is_refused() {
    let message = refusal(&edited(WITH_EFFECT, "{attempt}", "{pid}"));
    assert!(
        message.contains("pid"),
        "the refusal must name the placeholder it does not know: {message}"
    );
}
