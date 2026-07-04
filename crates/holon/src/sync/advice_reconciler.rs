//! The advice-rule reconciler: discovery CDC → live matview DDL (ADR 0022 A1).
//!
//! An advice rule is a vault block (`source_language =
//! 'holon_advice_rule_yaml'`) discovered by the profile-pattern scan
//! ([`holon_advice::GET_ADVICE_RULES_SQL`]). When a rule block appears /
//! changes / is deleted, the engine must synthesize, diff, or tear down the
//! rule's `advice_rule_{slug}` matview.
//!
//! **DDL never runs inline in the CDC delivery path.** Running `CREATE/DROP
//! MATERIALIZED VIEW` from inside the task that drains the CDC stream deadlocks
//! the Turso `DatabaseActor` (it starves waiting on the very actor the DDL call
//! blocks on — the `turso_storage_pbt` deadlock precedent). So this splits into
//! two tasks:
//!
//! 1. **Drainer** — reads the discovery matview's CDC stream and does only
//!    cheap work: convert each `Change` into a [`RuleEvent`] and forward it
//!    over an mpsc channel. Never touches the DB.
//! 2. **Reconciler** — owns the `DbHandle`, receives events sequentially,
//!    parses the YAML, decides via the pure [`AdviceReconcilerState`], and runs
//!    the DROP/reconcile DDL one event at a time. Records the outcome in the
//!    [`AdviceRuleStatusHandle`].
//!
//! Initial rows are pushed through the same channel *before* the stream drainer
//! starts, so the reconciler sees them first (ordering).

use holon_advice::AdviceReconcilerState;
use holon_advice::AdviceRuleStatus;
use holon_advice::AdviceRuleStatusHandle;
use holon_advice::ReconcilePlan;
use holon_advice::RuleEvent;
use holon_advice::StatusOutcome;
use holon_advice::parse_advice_rule;
use holon_advice::reconcile_advice_rule;
use holon_api::StorageEntity;
use holon_api::streaming::ActorAbortGuard;
use holon_api::streaming::Change;
use tokio::sync::mpsc;
use tokio_stream::StreamExt;

use crate::storage::turso::DbHandle;
use crate::sync::MatviewManager;

/// Keeps the reconciler's two background tasks alive. Dropping it aborts them
/// (mirrors `LinkEventSubscriberHandle` / the profile watcher staying alive by
/// being held on the engine).
pub struct AdviceReconcilerHandle {
    _aborts: ActorAbortGuard,
}

/// Distil a CDC `Change` over the `(id, content)` discovery view into a
/// [`RuleEvent`].
///
/// `Created`/`Updated` carry the row (id + content); `Deleted` carries only the
/// id — which is exactly what [`RuleEvent::Deleted`] needs. `FieldsChanged` is
/// not emitted by this matview shape; if one ever arrives it is surfaced
/// loudly, not silently dropped.
fn row_to_event(data: StorageEntity) -> Option<RuleEvent> {
    let block_id = data.get("id").and_then(|v| v.as_string())?.to_string();
    let content = data
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or_default()
        .to_string();
    Some(RuleEvent::Upserted { block_id, content })
}

fn change_to_event(change: Change<StorageEntity>) -> Option<RuleEvent> {
    match change {
        Change::Created { data, .. } | Change::Updated { data, .. } => row_to_event(data),
        Change::Deleted { id, .. } => Some(RuleEvent::Deleted { block_id: id }),
        Change::FieldsChanged { entity_id, .. } => {
            tracing::error!(
                %entity_id,
                "[AdviceReconciler] unexpected FieldsChanged on the advice-rule \
                 discovery view — the (id, content) matview should emit whole rows"
            );
            None
        }
    }
}

/// Execute one plan against the live DB, recording status. DDL failures are
/// surfaced (status + `tracing::error`), never swallowed.
async fn apply_plan(db_handle: &DbHandle, status: &AdviceRuleStatusHandle, plan: ReconcilePlan) {
    // Drops first (rename / teardown), so a rename can't briefly have two live
    // views.
    let mut ddl_error: Option<String> = None;
    for slug in &plan.drops {
        let view_name = slug.matview_name();
        if let Err(e) = db_handle
            .execute_ddl(&format!("DROP VIEW IF EXISTS {view_name}"))
            .await
        {
            tracing::error!(%view_name, error = %format!("{e:#}"), "[AdviceReconciler] DROP failed");
            ddl_error = Some(format!("dropping {view_name}: {e:#}"));
        }
    }

    if let Some(rule) = &plan.reconcile {
        match reconcile_advice_rule(db_handle, rule).await {
            Ok(outcome) => {
                tracing::info!(
                    slug = rule.name.as_str(),
                    ?outcome,
                    "[AdviceReconciler] reconciled advice rule"
                );
            }
            Err(e) => {
                tracing::error!(
                    slug = rule.name.as_str(),
                    error = %format!("{e:#}"),
                    "[AdviceReconciler] matview synthesis/reconcile failed"
                );
                ddl_error = Some(format!("{e:#}"));
            }
        }
    }

    match plan.status {
        StatusOutcome::Clear { block_id } => status.clear(&block_id),
        StatusOutcome::Set {
            block_id,
            status: intended,
        } => {
            // A DDL failure downgrades the intended `Active` to a visible `DdlError`.
            let recorded = match ddl_error {
                Some(msg) => AdviceRuleStatus::DdlError(msg),
                None => intended,
            };
            status.set(block_id, recorded);
        }
    }
}

/// Discover advice-rule blocks and keep their matviews reconciled with live
/// edits.
///
/// Returns a handle that must be held (on the engine) for the reconciler to
/// keep running. Watches [`holon_advice::GET_ADVICE_RULES_SQL`] via
/// `matview_manager` exactly like the profile resolver watches `PROFILE_SQL`.
pub async fn spawn_advice_reconciler(
    matview_manager: &MatviewManager,
    db_handle: DbHandle,
    status: AdviceRuleStatusHandle,
) -> anyhow::Result<AdviceReconcilerHandle> {
    let watch = matview_manager
        .watch(holon_advice::GET_ADVICE_RULES_SQL)
        .await?;

    let (event_tx, mut event_rx) = mpsc::channel::<RuleEvent>(64);
    let mut aborts = ActorAbortGuard::new();

    // Reconciler task: owns the DbHandle, runs DDL sequentially.
    let reconciler = tokio::spawn(async move {
        let mut state = AdviceReconcilerState::new();
        while let Some(event) = event_rx.recv().await {
            let plan = state.plan(event, parse_advice_rule);
            apply_plan(&db_handle, &status, plan).await;
        }
    });
    aborts.push(reconciler.abort_handle());

    // Feed initial rows through the SAME channel first (ordering before the
    // stream).
    for row in watch.initial_rows {
        if let Some(event) = row_to_event(row) {
            if event_tx.send(event).await.is_err() {
                return Ok(AdviceReconcilerHandle { _aborts: aborts });
            }
        }
    }

    // Drainer task: CDC stream → RuleEvent → channel. No DB work here.
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

    Ok(AdviceReconcilerHandle { _aborts: aborts })
}
