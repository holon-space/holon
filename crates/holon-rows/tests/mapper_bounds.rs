//! What a sidecar's jaq mapping may do to the process that runs it.
//!
//! A mapping is code from a file the user installed, run against bytes a remote
//! peer chose. It gets the pure part of jq's standard library and a bounded
//! output stream; it does not get the process, its environment or its log.

use std::panic::AssertUnwindSafe;
use std::process::Command;
use std::process::Output;

use holon_rows::MAX_MAPPING_OUTPUT_BYTES;
use holon_rows::MAX_MAPPING_OUTPUTS;
use holon_rows::RowMapper;
use serde_json::json;

const LABEL: &str = "fixture: holon.tools.pull.response";

/// Set in a child test process; the payload runs only there, so a payload that
/// ends its process ends the child, never the harness.
const CHILD: &str = "HOLON_ROWS_MAPPER_BOUNDS_CHILD";
const OUTCOME: &str = "CHILD-OUTCOME: ";

fn run_child(test: &str, envs: &[(&str, &str)]) -> Output {
    Command::new(std::env::current_exe().expect("the test binary has a path"))
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .env(CHILD, "1")
        .envs(envs.iter().copied())
        .output()
        .expect("the child test process starts")
}

fn report(outcome: anyhow::Result<Vec<serde_json::Value>>) {
    match outcome {
        Ok(values) => println!("{OUTCOME}ok {values:?}"),
        Err(e) => println!("{OUTCOME}err {}", format!("{e:#}").replace('\n', " ")),
    }
}

fn child_outcome(out: &Output) -> String {
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "the mapping ended the process that ran it ({}); stdout: {stdout}",
        out.status
    );
    let (_, outcome) = stdout
        .split_once(OUTCOME)
        .unwrap_or_else(|| panic!("the child reported no outcome; stdout: {stdout}"));
    outcome.lines().next().unwrap_or_default().to_string()
}

/// The error a stream must end in; an accepted stream is reported by its size,
/// since printing a stream built to exceed a cap would itself be the flood.
fn refused(source: &str) -> String {
    let mapped = RowMapper::compile(LABEL, source)
        .expect("compiles")
        .map(&json!(null));
    match mapped {
        Ok(values) => panic!(
            "`{source}` was accepted: {} values, {} bytes of JSON",
            values.len(),
            values.iter().map(|v| v.to_string().len()).sum::<usize>()
        ),
        Err(e) => format!("{e:#}"),
    }
}

#[test]
fn halt_error_in_a_mapping_is_an_error_and_the_process_survives() {
    if std::env::var_os(CHILD).is_some() {
        report(
            RowMapper::compile(
                LABEL,
                r#"if .version == null then "no version" | halt_error else . end"#,
            )
            .and_then(|m| m.map(&json!({}))),
        );
        return;
    }
    let outcome = child_outcome(&run_child(
        "halt_error_in_a_mapping_is_an_error_and_the_process_survives",
        &[],
    ));
    assert!(
        outcome.starts_with("err ") && outcome.contains("`halt_error`") && outcome.contains(LABEL),
        "a mapping calling halt_error must be an error naming halt_error and the mapping; got: \
         {outcome}"
    );
}

#[test]
fn env_in_a_mapping_is_refused_and_reads_nothing() {
    const PROBE: &str = "HOLON_ROWS_MAPPER_ENV_PROBE";
    if std::env::var_os(CHILD).is_some() {
        report(
            RowMapper::compile(LABEL, &format!("env | .{PROBE}")).and_then(|m| m.map(&json!(null))),
        );
        return;
    }
    let outcome = child_outcome(&run_child(
        "env_in_a_mapping_is_refused_and_reads_nothing",
        &[(PROBE, "probe-value")],
    ));
    assert!(
        !outcome.contains("probe-value"),
        "a mapping read the process environment: {outcome}"
    );
    assert!(
        outcome.starts_with("err ") && outcome.contains("`env`") && outcome.contains(LABEL),
        "a mapping calling env must be refused naming env and the mapping; got: {outcome}"
    );
}

/// Each source is refused when it compiles, so none of these runs here.
#[test]
fn process_environment_clock_and_log_builtins_are_refused_at_compile() {
    for (source, name) in [
        ("halt", "halt"),
        ("halt(0)", "halt"),
        ("halt_error", "halt_error"),
        ("halt_error(1)", "halt_error"),
        ("env", "env"),
        ("debug", "debug"),
        (r#"debug("msg")"#, "debug"),
        ("stderr", "stderr"),
        ("debug_empty", "debug_empty"),
        ("stderr_empty", "stderr_empty"),
        ("now", "now"),
        ("localtime", "localtime"),
        (r#"strflocaltime("%Y")"#, "strflocaltime"),
        ("$ENV", "$ENV"),
        ("input", "input"),
        ("inputs", "inputs"),
    ] {
        let err = RowMapper::compile(LABEL, source)
            .err()
            .unwrap_or_else(|| panic!("`{source}` compiled, but a mapping may not call `{name}`"));
        let msg = format!("{err:#}");
        assert!(
            msg.contains(&format!("`{name}`")) && msg.contains(LABEL),
            "the refusal of `{source}` must name `{name}` and the mapping; got: {msg}"
        );
    }
}

const IMPORTS: &[&str] = &[
    r#"import "x" as $x; $x"#,
    r#"import "x" as $x; ."#,
    r#"import "x" as x; ."#,
    r#"include "x"; ."#,
];

#[test]
fn a_mapping_the_load_check_accepts_does_not_panic_when_it_runs() {
    for source in IMPORTS.iter().chain(&["$__loc__", "$__prog_args", "$ENV"]) {
        let Ok(mapper) = RowMapper::compile(LABEL, source) else {
            continue;
        };
        let ran = std::panic::catch_unwind(AssertUnwindSafe(|| mapper.map(&json!(null))));
        assert!(
            ran.is_ok(),
            "`{source}` passed the load check and panicked when it ran"
        );
    }
}

#[test]
fn module_and_data_imports_are_refused_at_load_with_the_reason() {
    for source in IMPORTS {
        let err = RowMapper::compile(LABEL, source)
            .err()
            .unwrap_or_else(|| panic!("`{source}` compiled, but a mapping may not import"));
        let msg = format!("{err:#}");
        assert!(
            msg.contains("`x` is not available to a mapping: ") && msg.contains(LABEL),
            "the refusal of `{source}` must name the import, the reason and the mapping; got: \
             {msg}"
        );
    }
}

#[test]
fn the_pure_library_a_mapping_uses_still_compiles_and_runs() {
    let mapper = RowMapper::compile(
        LABEL,
        r#"[ (.s | split("_")[0]), (.s | explode | map(select(. != 95)) | implode),
             (.n | floor | tostring), ([3,1,2] | unique), ({a:1} | to_entries[0].key),
             ([{k:1},{k:1}] | group_by(.k) | length), (.s | @uri), (.s | test("a.b")),
             (0 | todate), ("1970-01-01T00:00:00Z" | fromdate), (.s | ascii_upcase) ]"#,
    )
    .expect("the pure library compiles");
    let out = mapper
        .map(&json!({"s": "a_b", "n": 2.5}))
        .expect("the pure library runs");
    assert_eq!(
        out,
        vec![json!([
            "a",
            "ab",
            "2",
            [1, 2, 3],
            "a",
            1,
            "a_b",
            true,
            "1970-01-01T00:00:00Z",
            0,
            "A_B"
        ])]
    );
}

#[test]
fn a_stream_past_the_output_count_cap_is_an_error_naming_the_cap() {
    let at_cap = RowMapper::compile(LABEL, &format!("range({MAX_MAPPING_OUTPUTS})"))
        .expect("compiles")
        .map(&json!(null))
        .expect("a stream of exactly the cap is accepted");
    assert_eq!(at_cap.len(), MAX_MAPPING_OUTPUTS);

    let msg = refused(&format!("range({})", MAX_MAPPING_OUTPUTS + 1));
    assert!(
        msg.contains("MAX_MAPPING_OUTPUTS") && msg.contains(LABEL),
        "the error must name the output-count cap and the mapping; got: {msg}"
    );
}

#[test]
fn a_stream_past_the_output_size_cap_is_an_error_naming_the_cap() {
    const MIB: usize = 1024 * 1024;
    let values = MAX_MAPPING_OUTPUT_BYTES / MIB + 1;
    let msg = refused(&format!(r#"range({values}) | "x" * {MIB}"#));
    assert!(
        msg.contains("MAX_MAPPING_OUTPUT_BYTES") && msg.contains(LABEL),
        "the error must name the output-size cap and the mapping; got: {msg}"
    );
}
