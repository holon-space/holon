//! C4 derived-field SIDECAR — the reactive half: a CDC watcher
//! (`spawn_derived_field_reconciler`) maintains the `block_derived` table
//! incrementally from base-row changes.
//!
//! Proves the two contracts the ruling puts on the sidecar:
//!   1. O(delta): editing ONE block rewrites only that block's sidecar rows —
//!      NOT a full-table sweep. Verified by corrupting a sibling's sidecar row
//!      by hand and showing an unrelated edit leaves the corruption in place.
//!   2. Retraction: deleting a base row drops its sidecar rows.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::Value;
use holon_api::computation::ArithOp;
use holon_api::computation::Computation;
use holon_api::computation::ComputeError;
use holon_api::computation::DerivedField;
use holon_api::computation::FieldIdent;
use holon_turso::derived_reconciler::spawn_derived_field_reconciler;
use holon_turso::matview_manager::MatviewManager;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockDerivedSchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use proptest::prelude::*;

async fn setup() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend); // keep the actor alive for the test
    handle
        .execute_ddl("CREATE TABLE task (id TEXT PRIMARY KEY, priority INTEGER)")
        .await
        .expect("create base table");
    BlockDerivedSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block_derived sidecar table");
    handle
}

async fn set_priority(handle: &DbHandle, id: &str, priority: i64) {
    handle
        .execute(
            "INSERT INTO task (id, priority) VALUES (?, ?) ON CONFLICT(id) DO UPDATE SET priority \
             = excluded.priority",
            vec![
                turso::Value::Text(id.into()),
                turso::Value::Integer(priority),
            ],
        )
        .await
        .expect("upsert task");
}

/// Current `value_json` of one sidecar row, or `None` if absent.
async fn read_derived(handle: &DbHandle, block_id: &str, field: &str) -> Option<String> {
    let rows = handle
        .query_positional(
            "SELECT value_json FROM block_derived WHERE block_id = ? AND field_name = ?",
            vec![
                turso::Value::Text(block_id.into()),
                turso::Value::Text(field.into()),
            ],
        )
        .await
        .expect("query block_derived");
    rows.first().map(|r| match r.get("value_json") {
        Some(Value::String(s)) => s.clone(),
        other => panic!("value_json: unexpected {other:?}"),
    })
}

/// Poll `read_derived` until it equals `expected`, or fail after a bounded wait
/// (the watcher is asynchronous).
async fn await_derived(handle: &DbHandle, block_id: &str, field: &str, expected: Option<&str>) {
    for _ in 0..100 {
        if read_derived(handle, block_id, field).await.as_deref() == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "sidecar {block_id}.{field} never reached {expected:?}; last = {:?}",
        read_derived(handle, block_id, field).await
    );
}

/// `boosted = priority * 2` (SQL-plantable, but the sidecar path evaluates it
/// in Rust uniformly).
fn boosted() -> DerivedField {
    DerivedField::new(
        FieldIdent::parse("boosted").expect("identifier"),
        Computation::Arith {
            op: ArithOp::Mul,
            lhs: Box::new(Computation::Field("priority".into())),
            rhs: Box::new(Computation::Lit(Value::Integer(2))),
        },
    )
}

fn expected_json(priority: i64) -> String {
    let ctx: HashMap<String, Value> =
        HashMap::from([("priority".to_string(), Value::Integer(priority))]);
    let v = boosted().computation.eval(&ctx).expect("eval");
    serde_json::to_string(&v).expect("json")
}

#[tokio::test]
async fn sidecar_is_cdc_maintained_incrementally_and_retracts() {
    let handle = setup().await;
    let mgr = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));

    // Seed BEFORE spawning: t1, t2 arrive as the watcher's initial snapshot.
    set_priority(&handle, "t1", 3).await;
    set_priority(&handle, "t2", 1).await;

    let _handle_guard = spawn_derived_field_reconciler(
        &mgr,
        handle.clone(),
        "SELECT id, priority FROM task",
        vec![boosted()],
        Arc::new(ConditionBus::new()),
    )
    .await
    .expect("spawn reconciler");

    // Initial maintenance: both blocks get their derived row.
    await_derived(&handle, "t1", "boosted", Some(&expected_json(3))).await;
    await_derived(&handle, "t2", "boosted", Some(&expected_json(1))).await;

    // O(DELTA) PROBE: hand-corrupt t2's sidecar row. If the watcher did a
    // full-table sweep on ANY edit, an unrelated t1 edit would overwrite this
    // back to the correct value.
    handle
        .execute(
            "UPDATE block_derived SET value_json = ? WHERE block_id = 't2' AND field_name = \
             'boosted'",
            vec![turso::Value::Text("\"CORRUPT\"".into())],
        )
        .await
        .expect("corrupt t2");

    // Edit ONLY t1. The CDC delta carries t1's row alone.
    set_priority(&handle, "t1", 5).await;
    await_derived(&handle, "t1", "boosted", Some(&expected_json(5))).await;

    // O(delta) proven: t2 was never recomputed, so the corruption stands.
    assert_eq!(
        read_derived(&handle, "t2", "boosted").await.as_deref(),
        Some("\"CORRUPT\""),
        "editing t1 must NOT sweep/recompute t2's sidecar row (O(delta) contract)"
    );

    // RETRACTION: deleting a base row drops its sidecar rows.
    handle
        .execute(
            "DELETE FROM task WHERE id = ?",
            vec![turso::Value::Text("t1".into())],
        )
        .await
        .expect("delete t1");
    await_derived(&handle, "t1", "boosted", None).await;
}

/// `ratio = priority / divisor`; a zero divisor makes it non-finite.
fn ratio(divisor: f64) -> DerivedField {
    DerivedField::new(
        FieldIdent::parse("ratio").expect("identifier"),
        Computation::Arith {
            op: ArithOp::Div,
            lhs: Box::new(Computation::Field("priority".into())),
            rhs: Box::new(Computation::Lit(Value::Float(divisor))),
        },
    )
}

/// `scaled = priority * f64::MAX`: finite at 1, infinite at 2.
fn scaled() -> DerivedField {
    DerivedField::new(
        FieldIdent::parse("scaled").expect("identifier"),
        Computation::Arith {
            op: ArithOp::Mul,
            lhs: Box::new(Computation::Field("priority".into())),
            rhs: Box::new(Computation::Lit(Value::Float(f64::MAX))),
        },
    )
}

/// The `DerivedFieldNotComputed` reason raised for `t.scaled`, if any.
fn not_computed_reason(bus: &ConditionBus) -> Option<String> {
    bus.current().into_iter().find_map(|c| match c.reason {
        ConditionKind::DerivedFieldNotComputed {
            block_id,
            field,
            reason,
        } if block_id == "t" && field == "scaled" => Some(reason),
        _ => None,
    })
}

#[tokio::test]
async fn a_value_turning_non_finite_drops_its_row_and_is_disclosed_until_it_recovers() {
    let handle = setup().await;
    let mgr = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
    let bus = Arc::new(ConditionBus::new());
    set_priority(&handle, "t", 1).await;
    let _guard = spawn_derived_field_reconciler(
        &mgr,
        handle.clone(),
        "SELECT id, priority FROM task",
        vec![boosted(), scaled()],
        bus.clone(),
    )
    .await
    .expect("spawn reconciler");
    let finite = serde_json::to_string(&Value::Float(f64::MAX)).expect("json");
    await_derived(&handle, "t", "scaled", Some(&finite)).await;
    assert_eq!(not_computed_reason(&bus), None);

    set_priority(&handle, "t", 2).await;
    await_derived(&handle, "t", "boosted", Some(&expected_json(2))).await;
    assert_eq!(
        read_derived(&handle, "t", "scaled").await,
        None,
        "the row computed from priority 1 must not stand as the value of priority 2"
    );
    let reason = not_computed_reason(&bus).expect("the missing value must be disclosed");
    assert!(reason.contains("non-finite float inf"), "{reason}");

    set_priority(&handle, "t", 1).await;
    await_derived(&handle, "t", "scaled", Some(&finite)).await;
    assert_eq!(
        not_computed_reason(&bus),
        None,
        "a successful computation clears the condition"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]

    /// A non-finite result leaves no row, never a stored `null`.
    #[test]
    fn a_non_finite_derived_value_never_lands_in_the_sidecar(
        priority in -3i64..=3,
        divisor in prop::sample::select(vec![0.0, -0.0, 0.5, 1e-320, 3.0]),
    ) {
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        runtime.block_on(async {
            let handle = setup().await;
            let mgr = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
            set_priority(&handle, "t", priority).await;
            let _guard = spawn_derived_field_reconciler(
                &mgr,
                handle.clone(),
                "SELECT id, priority FROM task",
                vec![boosted(), ratio(divisor)],
                Arc::new(ConditionBus::new()),
            )
            .await
            .expect("spawn reconciler");

            // Both fields are written in one transaction, so `boosted` landing
            // means `ratio` is decided too.
            await_derived(&handle, "t", "boosted", Some(&expected_json(priority))).await;

            let ctx = HashMap::from([("priority".to_string(), Value::Integer(priority))]);
            let value = ratio(divisor).computation.eval(&ctx);
            let expected = match &value {
                Ok(value) => Some(serde_json::to_string(value).expect("json")),
                Err(ComputeError::Arithmetic { .. }) => None,
                Err(e) => panic!("{priority} / {divisor:?} must evaluate or be non-finite: {e}"),
            };
            assert_eq!(
                read_derived(&handle, "t", "ratio").await,
                expected,
                "{priority} / {divisor:?} evaluates to {value:?}"
            );
        });
    }
}
