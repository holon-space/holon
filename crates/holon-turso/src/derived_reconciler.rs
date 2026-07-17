//! C4 derived-field SIDECAR reconciler — the reactive half of "sidecar
//! `block_derived` + CDC watcher".
//!
//! A CDC watcher over a *source view* (a matview yielding `id` plus the input
//! columns the declared computations read). For each row delta it recomputes
//! only the affected block's derived fields via
//! [`holon_api::computation::Computation::eval`] — the SAME evaluator
//! `rank_tasks` / the seat-B stage path use — and UPSERTs them into the
//! `block_derived` sidecar table. A block-Deleted delta retracts that block's
//! sidecar rows. Maintenance is O(delta): a single-block edit rewrites only
//! that block's rows, never a full-table sweep (the CDC stream carries only the
//! changed rows).
//!
//! Architectural template: `holon::sync::advice_reconciler` (the advice
//! weaver's canonical-read watcher). Two tasks — a *drainer* that maps CDC
//! deltas to events off the broadcast path, and a *reconciler* that owns the
//! [`DbHandle`] and performs the writes sequentially.
//!
//! ## Seat routing (relative to seat A)
//!
//! Seat A plants a SQL-compilable computation as an IVM matview column (wide,
//! inline on the `block` matview). This sidecar path evaluates in Rust and is
//! uniform across BOTH seats — a `Script` (seat B) field lands here identically
//! to an arithmetic one, and the value written always equals
//! `Computation::eval` by construction. Sourcing an already-planted column's
//! IVM value instead of re-evaluating it (the "prefer IVM when plantable"
//! optimization) is a deferred perf refinement, not a correctness change: seat
//! A already proves planted == eval.

use std::collections::hash_map::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;

use holon_api::Change;
use holon_api::Value;
use holon_api::computation::Context;
use holon_api::computation::DerivedField;
use holon_core::storage::StorageEntity;
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio_stream::StreamExt;

use crate::matview_manager::MatviewManager;
use crate::turso::DbHandle;
use crate::turso::value_to_turso_param;

/// A declared derived field paired with the provenance hash of its computation.
/// Provenance lets a value produced by an outdated declaration be detected: a
/// row whose stored `provenance` differs from the current declaration's is
/// stale.
#[derive(Clone)]
struct CompiledDerived {
    field: DerivedField,
    provenance: String,
}

/// A distilled sidecar-maintenance event, decoupled from the CDC batch shape so
/// the write task never touches the broadcast stream.
enum SidecarEvent {
    /// Recompute + upsert every declared field for this block from its row.
    Upsert(StorageEntity),
    /// Retract every sidecar row for this block.
    Delete(String),
}

/// Keeps the reconciler's two background tasks alive. Dropping it aborts them
/// (same lifetime contract as `AdviceReconcilerHandle`).
pub struct DerivedFieldReconcilerHandle {
    aborts: Vec<AbortHandle>,
}

impl Drop for DerivedFieldReconcilerHandle {
    fn drop(&mut self) {
        for abort in &self.aborts {
            abort.abort();
        }
    }
}

/// Provenance hash of a field's computation. A stable-within-build content hash
/// over the structural `Debug` of the [`holon_api::computation::Computation`] —
/// enough to detect a declaration change (different op / literal / field set
/// ⇒ different provenance). Cross-build stability is not required: the sidecar
/// is a rebuildable cache and provenance is only read for in-process staleness
/// detection.
fn provenance_of(field: &DerivedField) -> String {
    let mut hasher = DefaultHasher::new();
    format!("{:?}", field.computation).hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Build the named-value [`Context`] a computation evaluates against from a CDC
/// row (matview column-name → value).
fn row_to_context(row: &StorageEntity) -> Context {
    row.iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn change_to_event(change: Change<StorageEntity>) -> Option<SidecarEvent> {
    match change {
        Change::Created { data, .. } | Change::Updated { data, .. } => {
            Some(SidecarEvent::Upsert(data))
        }
        Change::Deleted { id, .. } => Some(SidecarEvent::Delete(id)),
        Change::FieldsChanged { entity_id, .. } => {
            tracing::error!(
                %entity_id,
                "[DerivedFieldReconciler] unexpected FieldsChanged on the source view — the \
                 sidecar source matview must emit whole rows"
            );
            None
        }
    }
}

/// Recompute every declared field for one block and upsert the results in a
/// single transaction. A computation that fails to evaluate (e.g. a declared
/// input column absent from the source row) is surfaced LOUDLY at `error`
/// level with full context and its row is skipped — never written as a faked
/// value.
async fn apply_upsert(db_handle: &DbHandle, fields: &[CompiledDerived], row: &StorageEntity) {
    let block_id = match row.get("id").and_then(|v| v.as_string()) {
        Some(id) => id.to_string(),
        None => {
            tracing::error!(
                ?row,
                "[DerivedFieldReconciler] source row has no string `id` column; cannot key sidecar"
            );
            return;
        }
    };
    let ctx = row_to_context(row);
    let mut statements: Vec<(String, Vec<turso::Value>)> = Vec::with_capacity(fields.len());
    for compiled in fields {
        let value = match compiled.field.computation.eval(&ctx) {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(
                    %block_id,
                    field = %compiled.field.name,
                    error = %e,
                    "[DerivedFieldReconciler] derived-field evaluation failed; skipping this \
                     field's sidecar write (no faked value)"
                );
                continue;
            }
        };
        let value_json = match serde_json::to_string(&value) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!(
                    %block_id,
                    field = %compiled.field.name,
                    error = %e,
                    "[DerivedFieldReconciler] could not JSON-encode derived value"
                );
                continue;
            }
        };
        statements.push((
            "INSERT INTO block_derived (block_id, field_name, value_json, provenance) VALUES (?, \
             ?, ?, ?) ON CONFLICT(block_id, field_name) DO UPDATE SET value_json = \
             excluded.value_json, provenance = excluded.provenance"
                .to_string(),
            vec![
                value_to_turso_param(&Value::String(block_id.clone())),
                value_to_turso_param(&Value::String(compiled.field.name.clone())),
                value_to_turso_param(&Value::String(value_json)),
                value_to_turso_param(&Value::String(compiled.provenance.clone())),
            ],
        ));
    }
    if statements.is_empty() {
        return;
    }
    if let Err(e) = db_handle.transaction(statements).await {
        tracing::error!(
            %block_id,
            error = %e,
            "[DerivedFieldReconciler] sidecar upsert transaction failed"
        );
    }
}

/// Retract every sidecar row for a deleted block.
async fn apply_delete(db_handle: &DbHandle, block_id: &str) {
    if let Err(e) = db_handle
        .transaction(vec![(
            "DELETE FROM block_derived WHERE block_id = ?".to_string(),
            vec![value_to_turso_param(&Value::String(block_id.to_string()))],
        )])
        .await
    {
        tracing::error!(
            %block_id,
            error = %e,
            "[DerivedFieldReconciler] sidecar retraction transaction failed"
        );
    }
}

/// Spawn the derived-field sidecar reconciler.
///
/// Watches `source_view_sql` (a SELECT yielding `id` plus every column the
/// `fields'` computations read) via `matview_manager`, and keeps
/// `block_derived` in sync with it. Returns a handle that MUST be held for the
/// reconciler to keep running.
///
/// `fields` is the block type's declared derived-field set. Sourcing it from a
/// declaration surface (Martin's escalated item 1) is out of scope here — the
/// production caller injects it once that ruling lands; today it is passed in
/// directly (the same shape `DerivedFieldPlan::plan` already consumes).
pub async fn spawn_derived_field_reconciler(
    matview_manager: &MatviewManager,
    db_handle: DbHandle,
    source_view_sql: &str,
    fields: Vec<DerivedField>,
) -> anyhow::Result<DerivedFieldReconcilerHandle> {
    let compiled: Vec<CompiledDerived> = fields
        .into_iter()
        .map(|field| {
            let provenance = provenance_of(&field);
            CompiledDerived { field, provenance }
        })
        .collect();

    let watch = matview_manager.watch(source_view_sql).await?;

    let (event_tx, mut event_rx) = mpsc::channel::<SidecarEvent>(64);
    let mut aborts = Vec::new();

    // Reconciler task: owns the DbHandle, performs writes sequentially.
    let reconciler = tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                SidecarEvent::Upsert(row) => apply_upsert(&db_handle, &compiled, &row).await,
                SidecarEvent::Delete(id) => apply_delete(&db_handle, &id).await,
            }
        }
    });
    aborts.push(reconciler.abort_handle());

    // Feed the initial snapshot through the SAME channel first (ordering before
    // the live stream), mirroring the advice reconciler.
    for row in watch.initial_rows {
        if event_tx.send(SidecarEvent::Upsert(row)).await.is_err() {
            return Ok(DerivedFieldReconcilerHandle { aborts });
        }
    }

    // Drainer task: CDC stream → SidecarEvent → channel. No DB work here, so the
    // broadcast receiver never lags behind slow writes.
    let mut stream = watch.stream;
    let drainer = tokio::spawn(async move {
        while let Some(batch) = stream.next().await {
            for row_change in batch.inner.items {
                if let Some(event) = change_to_event(row_change.change) {
                    if event_tx.send(event).await.is_err() {
                        return;
                    }
                }
            }
        }
    });
    aborts.push(drainer.abort_handle());

    Ok(DerivedFieldReconcilerHandle { aborts })
}
