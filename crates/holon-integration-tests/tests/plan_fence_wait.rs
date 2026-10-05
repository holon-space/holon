//! **How long a local write waits behind an agent's `dense_patch`.**
//!
//! A judged plan claims the rows it names from its judgement to its last op.
//! This measures what a local write on another row still waits: while the
//! real `dense_patch` tool retitles N rows over the wire, the user's writes run
//! back to back on the same engine, and the slowest of them, less the median
//! of the same write alone, is the wait the patch added.
//!
//! A measurement, not a gate: `#[ignore]`, run with `--ignored --nocapture`
//! and read the `[fence wait]` lines.

use std::cell::Cell;
use std::collections::HashMap;
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OpOrigin;
use holon_api::SourceKeystroke;
use holon_api::Value;
use holon_api::block::Block;
use holon_integration_tests::McpUserDriver;
use holon_integration_tests::pbt::composed::embedded_mcp::EmbeddedMcp;
use holon_integration_tests::pbt::composed::embedded_mcp::connect_embedded_mcp;
use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::transitions::WriteOrgFile;
use holon_org_format::OrgBlockExt;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

type Sut = ComposedSut<WideE2E>;

/// The most rows one `dense_patch` writes.
const ROWS: usize = 40;
const REPEATS: usize = 5;
const QUERY: &str = "SELECT * FROM block WHERE parent_id = 'block:fw-home' ORDER BY sort_key";
const LOCAL: &str = "block:fw-local";

fn heading(id: &str, parent: &str, content: &str) -> Block {
    let mut b = Block::new_text(EntityUri::block(id), EntityUri::block(parent), content);
    b.set_property("ID", Value::String(id.to_string()));
    b
}

/// N rows for the agent under one heading; the user's block under another, so
/// no write of the user touches the projection the agent patches.
fn boot() -> (Sut, EmbeddedMcp) {
    holon_integration_tests::pbt::set_loro_peer_id_if_unset("1");
    let mut blocks = vec![heading("fw-home", "gen-placeholder", "Agent rows")];
    blocks
        .extend((0..ROWS).map(|j| heading(&format!("fw-{j}"), "fw-home", &format!("Row {j} v0"))));
    blocks.push(heading("fw-user", "gen-placeholder", "User rows"));
    blocks.push(heading("fw-local", "fw-user", "Typed 0"));
    let t = E2ETransition::WriteOrgFile(WriteOrgFile {
        filename: "fence_wait.org".to_string(),
        blocks,
        keyword_set: None,
    });
    let ref_state = wide_e2e_ref();
    let sut = Sut::init_test(&ref_state);
    let mut ref_state: ReferenceState = ref_state;
    assert!(WideE2EMachine::preconditions(&ref_state, &t));
    ref_state = WideE2EMachine::apply(ref_state, &t);
    let sut = <Sut as StateMachineTest>::apply(sut, &ref_state, t);
    sut.settle_projections();
    let driver = connect_embedded_mcp(&sut, "plan-fence-wait");
    (sut, driver)
}

#[derive(Clone, Copy, Debug)]
enum LocalWrite {
    Keystroke,
    SetField,
}

async fn local_write(sut: &Sut, kind: LocalWrite, n: usize) -> Duration {
    let engine = sut
        .handle()
        .engine()
        .expect("full_headless boots a Turso engine");
    let start = Instant::now();
    match kind {
        LocalWrite::Keystroke => {
            engine
                .commit_keystroke(SourceKeystroke {
                    id: LOCAL.into(),
                    source: format!("Typed {n}"),
                    write_seq: None,
                })
                .await
                .expect("the keystroke lands");
        }
        LocalWrite::SetField => {
            let params = HashMap::from([
                ("id".into(), Value::String(LOCAL.into())),
                ("field".into(), Value::String("content".into())),
                ("value".into(), Value::String(format!("Typed {n}"))),
            ]);
            engine
                .execute_operation(
                    &EntityName::new("block"),
                    "set_field",
                    params,
                    OpOrigin::User,
                )
                .await
                .expect("the set_field lands");
        }
    }
    start.elapsed()
}

async fn projection(driver: &McpUserDriver) -> (String, String) {
    let proj = driver
        .call_tool_json(
            "dense_query",
            serde_json::json!({ "query": QUERY, "language": "holon_sql" }),
        )
        .await
        .expect("dense_query over MCP");
    (
        proj["projection_handle"]
            .as_str()
            .expect("a handle")
            .to_string(),
        proj["dense_org"].as_str().expect("dense_org").to_string(),
    )
}

/// Retitle rows `0..n` to their next version.
fn retitle(dense: &str, version: &mut [usize]) -> String {
    version
        .iter_mut()
        .enumerate()
        .fold(dense.to_string(), |text, (j, v)| {
            let from = format!("* Row {j} v{v} {{");
            assert!(text.contains(&from), "{from:?} is not in:\n{text}");
            *v += 1;
            text.replacen(&from, &format!("* Row {j} v{v} {{"), 1)
        })
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

#[test]
#[ignore = "measurement; run with --ignored --nocapture"]
fn local_writes_wait_behind_an_agents_dense_patch() {
    let (sut, mcp) = boot();
    let driver = &mcp;
    let mut written = 0usize;
    let mut version = [0usize; ROWS];
    for kind in [LocalWrite::Keystroke, LocalWrite::SetField] {
        let alone = median(sut.runtime().block_on(async {
            let mut v = Vec::new();
            for _ in 0..20 {
                written += 1;
                v.push(local_write(&sut, kind, written).await);
            }
            v
        }));
        for n in [1usize, 10, ROWS] {
            for _ in 0..REPEATS {
                sut.settle_projections();
                let done = Cell::new(false);
                let (patch, locals) = sut.runtime().block_on(async {
                    let (handle, dense) = projection(driver).await;
                    let edited = retitle(&dense, &mut version[..n]);
                    let patch = async {
                        let start = Instant::now();
                        driver
                            .call_tool_json(
                                "dense_patch",
                                serde_json::json!({ "handle": handle, "text": edited }),
                            )
                            .await
                            .expect("the patch applies");
                        let took = start.elapsed();
                        done.set(true);
                        took
                    };
                    let locals = async {
                        let mut v = Vec::new();
                        while !done.get() {
                            written += 1;
                            v.push(local_write(&sut, kind, written).await);
                        }
                        v
                    };
                    tokio::join!(patch, locals)
                });
                let worst = locals.iter().max().copied().unwrap_or_default();
                eprintln!(
                    "[fence wait] {kind:?} N={n}: patch {:.1} ms; {} local write(s) during it, \
                     slowest {:.1} ms, alone median {:.1} ms, added wait {:.1} ms",
                    ms(patch),
                    locals.len(),
                    ms(worst),
                    ms(alone),
                    ms(worst.saturating_sub(alone)),
                );
            }
        }
    }
}
