//! What the shape gate costs one write, reading the pre-write state from the
//! SQL write authority: on a block outside every tagged shape, in a list of
//! 1, 100 and 400 siblings, and on a decision.
//!
//! Run in RELEASE: `cargo test -p holon --release --features holon/test-helpers
//! --test shape_gate_cost -- --ignored --nocapture`.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::core::sql_operation_provider::SqlOperationProvider;
use holon::core::sql_write_authority::SqlWriteAuthority;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_core::ShapeValidators;
use holon_core::shape_gate::PlanOp;
use holon_core::shape_gate::judge_plan;
use holon_core::storage::types::StorageEntity;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;

/// Siblings of the measured plain block, which is its page's last child: a
/// lone indent reads its previous sibling, a read over this list.
const SIBLINGS: [usize; 3] = [1, 100, 400];
const SAMPLES: usize = 300;

fn s(v: &str) -> Value {
    Value::String(v.to_string())
}

fn params(pairs: &[(&str, Value)]) -> StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| (Arc::from(*k), v.clone()))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn gate_cost_on_a_plain_block_over_the_sql_authority() {
    let engine = create_test_engine_with_providers(":memory:".into(), |module| {
        module.with_operation_provider_factory(|backend| {
            let db_handle =
                tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
            Arc::new(SqlOperationProvider::with_edge_fields(
                db_handle,
                BLOCK_WRITE_TABLE.to_string(),
                "block".to_string(),
                "block".to_string(),
                BlockSchemaModule.edge_fields(),
            )) as Arc<dyn OperationProvider>
        })
    })
    .await
    .expect("test engine");
    let create = |id: String, parent: &str, sort_key: String| {
        params(&[
            ("id", s(&id)),
            ("parent_id", s(parent)),
            ("content", s("a note")),
            ("sort_key", s(&sort_key)),
        ])
    };
    let run = async |p: StorageEntity| {
        engine
            .execute_operation(&EntityName::new("block"), "create", p, OpOrigin::Sync)
            .await
            .expect("seed");
    };
    for page in ["block:page", "block:dpage"] {
        run(create(page.into(), "sentinel:no_parent", "A0".into())).await;
    }
    let mut decision = create("block:d0".into(), "block:dpage", "A0".into());
    decision.insert("tags".into(), Value::Array(vec![s("decision")]));
    decision.insert("task_state".into(), s("?"));
    run(decision).await;
    for key in ["a", "b", "c"] {
        let mut option = create(format!("block:d0-{key}"), "block:d0", format!("A{key}"));
        option.insert(
            "properties".into(),
            Value::Object([("option".to_string(), s(key))].into_iter().collect()),
        );
        run(option).await;
    }
    let authority = Arc::new(SqlWriteAuthority::new(engine.db_handle().clone()));
    let validators = ShapeValidators::registered();
    let mut created = 0;
    for siblings in SIBLINGS {
        while created <= siblings {
            run(create(
                format!("block:n{created}"),
                "block:page",
                format!("a{created:05}"),
            ))
            .await;
            created += 1;
        }
        for (name, op_name, p) in &cases(&format!("block:n{siblings}")) {
            let ops = [PlanOp { op_name, params: p }];
            let mut samples: Vec<Duration> = Vec::with_capacity(SAMPLES);
            for _ in 0..SAMPLES {
                let t = Instant::now();
                let judged = judge_plan(&validators, authority.clone(), &ops)
                    .await
                    .expect("reads");
                samples.push(t.elapsed());
                judged.verdict.expect("every case is a legal write");
            }
            samples.sort();
            eprintln!(
                "[shape-gate cost/sql] siblings={siblings} {name}: p50={:?} p95={:?} max={:?} \
                 ({SAMPLES} samples) load: {}",
                samples[SAMPLES / 2],
                samples[SAMPLES * 95 / 100],
                samples[SAMPLES - 1],
                load_average()
            );
        }
    }
}

/// The machine's load averages, as `uptime` prints them.
fn load_average() -> String {
    let out = std::process::Command::new("uptime")
        .output()
        .expect("uptime runs");
    let text = String::from_utf8(out.stdout).expect("uptime prints text");
    let at = text
        .find("load average")
        .unwrap_or_else(|| panic!("uptime printed no load average: {text}"));
    text[at..].trim().to_string()
}

fn cases(last: &str) -> Vec<(&'static str, &'static str, StorageEntity)> {
    vec![
        (
            "set_field(content), text-only",
            "set_field",
            params(&[("id", s(last)), ("field", s("content")), ("value", s("x"))]),
        ),
        (
            "set_field(task_state), not tagged",
            "set_field",
            params(&[
                ("id", s(last)),
                ("field", s("task_state")),
                ("value", s("TODO")),
            ]),
        ),
        (
            "indent of the last sibling, not tagged",
            "indent",
            params(&[("id", s(last))]),
        ),
        (
            "set_field(choose) on a decision",
            "set_field",
            params(&[
                ("id", s("block:d0")),
                ("field", s("choose")),
                ("value", s("1")),
            ]),
        ),
    ]
}
