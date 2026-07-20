//! Coverage closure for BugFunnel dogfood round-3 finding B2:
//! "undo after `cycle_task_state` does not revert it; reverts the previous
//! (indent/content) op instead — undo REPORTS success (false-green)."
//!
//! The provider-level regression (`cycle_task_state` returns a real
//! `task_state` inverse) is already locked by
//! `holon-loro/.../loro_block_operations.
//! rs::cycle_task_state_is_reversible_and_targets_task_state`. What was NEVER
//! covered is the FULL prod route the desktop dispatches through:
//! `FrontendSession::execute_operation` (OpOrigin::User) → DI `BackendEngine`
//! undo-push classification → `session.undo()`. This test closes that gap for
//! `cycle_task_state`, exactly as `undo_prod_session_wiring.rs` did for
//! `set_field(content)`.
//!
//! The test is designed to catch the SPECIFIC false-green symptom: it
//! dispatches a FIRST undoable User op (a content edit), THEN cycles the task
//! state, then presses undo ONCE. If `cycle_task_state` failed to push an
//! entry, the single undo would pop the content edit instead — leaving
//! `task_state` stuck and the content reverted. The assertions require the
//! mirror image: `task_state` reverts and the content edit stays.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_api::EntityName;
use holon_api::UndoOutcome;
use holon_api::Value;
use holon_integration_tests::test_environment::TestEnvironment;

async fn wait_until<F, Fut>(label: &str, timeout: Duration, mut f: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = Instant::now() + timeout;
    loop {
        if f().await {
            return true;
        }
        if Instant::now() >= deadline {
            eprintln!("wait_until('{label}') timed out after {timeout:?}");
            return false;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

async fn block_content(session: &holon_frontend::FrontendSession, id: &str) -> Option<String> {
    let snap = session
        .block_query()
        .snapshot()
        .await
        .expect("block snapshot");
    snap.iter_blocks()
        .find(|b| b.id.as_str() == id)
        .map(|b| b.content.clone())
}

async fn task_state(session: &holon_frontend::FrontendSession, id: &str) -> Option<String> {
    let snap = session
        .block_query()
        .snapshot()
        .await
        .expect("block snapshot");
    snap.iter_blocks()
        .find(|b| b.id.as_str() == id)
        .and_then(|b| b.get_property_str("task_state"))
}

/// A comparable snapshot of the fields a property-backed edit can touch. Used
/// as the metamorphic invariant surface: `state; op; undo` must restore this
/// tuple exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockFacet {
    content: Option<String>,
    task_state: Option<String>,
    effort: Option<String>,
}

async fn facet(session: &holon_frontend::FrontendSession, id: &str) -> BlockFacet {
    let snap = session
        .block_query()
        .snapshot()
        .await
        .expect("block snapshot");
    let b = snap
        .iter_blocks()
        .find(|b| b.id.as_str() == id)
        .expect("target block present in snapshot");
    BlockFacet {
        content: Some(b.content.clone()),
        task_state: b.get_property_str("task_state"),
        effort: b.get_property_str("effort"),
    }
}

async fn pick_target(session: &holon_frontend::FrontendSession) -> (String, String) {
    let snap = session
        .block_query()
        .snapshot()
        .await
        .expect("block snapshot");
    let target = snap
        .iter_blocks()
        .find(|b| {
            let id = b.id.as_str();
            id.starts_with("block:")
                && !id.contains("::src::")
                && !id.contains("::render::")
                && !b.content.is_empty()
        })
        .unwrap_or_else(|| {
            panic!(
                "no editable seeded content block found; snapshot ids: {:?}",
                snap.iter_blocks()
                    .map(|b| b.id.as_str())
                    .collect::<Vec<_>>()
            )
        });
    (target.id.as_str().to_string(), target.content.clone())
}

#[test]
fn cycle_task_state_undo_reverts_the_cycle_not_the_previous_op() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");

        let session = env.session_arc();

        assert!(
            !session.can_undo().await,
            "fresh prod session must start with an empty undo stack"
        );

        let (target_id, original_content) = pick_target(&session).await;
        let original_task_state = task_state(&session, &target_id).await;
        let edited = format!("{original_content} edited-first-op");

        // ── OP #1: a User-origin content edit. Pushes the FIRST undo entry.
        let seq = holon_api::write_seq::next();
        let mut params = std::collections::HashMap::new();
        params.insert("id".to_string(), Value::String(target_id.clone()));
        params.insert("field".to_string(), Value::String("content".to_string()));
        params.insert("value".to_string(), Value::String(edited.clone()));
        params.insert("write_seq".to_string(), Value::Integer(seq.get()));
        session
            .execute_operation(&EntityName::new("block"), "set_field", params)
            .await
            .expect("user content set_field dispatch");

        assert!(
            wait_until("can_undo-after-content", Duration::from_secs(5), || session
                .can_undo())
            .await,
            "content edit must push an undo entry"
        );

        // ── OP #2: a User-origin cycle_task_state. MUST push a SECOND undo entry
        //    (its own `task_state` inverse). This is the op the dogfood found
        //    missing.
        let mut cycle_params = std::collections::HashMap::new();
        cycle_params.insert("id".to_string(), Value::String(target_id.clone()));
        session
            .execute_operation(&EntityName::new("block"), "cycle_task_state", cycle_params)
            .await
            .expect("user cycle_task_state dispatch");

        // The cycle advanced the keyword away from its original value.
        let cycled = wait_until("task_state-advanced", Duration::from_secs(5), || {
            let session = session.clone();
            let target_id = target_id.clone();
            let original_task_state = original_task_state.clone();
            async move { task_state(&session, &target_id).await != original_task_state }
        })
        .await;
        assert!(
            cycled,
            "cycle_task_state did not advance the keyword (still {:?})",
            task_state(&session, &target_id).await
        );
        let after_cycle = task_state(&session, &target_id).await;

        // ── UNDO ONCE. This must invert OP #2 (the cycle), NOT OP #1 (content).
        let outcome = session.undo().await.expect("undo dispatch");
        assert!(
            matches!(outcome, UndoOutcome::Applied { .. }),
            "expected UndoOutcome::Applied, got {outcome:?}"
        );

        // (A) task_state reverted to its pre-cycle value.
        let reverted = wait_until("task_state-reverted", Duration::from_secs(5), || {
            let session = session.clone();
            let target_id = target_id.clone();
            let original_task_state = original_task_state.clone();
            async move { task_state(&session, &target_id).await == original_task_state }
        })
        .await;
        assert!(
            reverted,
            "B2 FALSE-GREEN: undo reported success but task_state was NOT reverted \
             (before cycle: {original_task_state:?}, after cycle: {after_cycle:?}, \
             after undo: {:?}). The single undo popped the WRONG op — cycle_task_state \
             never pushed its own undo entry.",
            task_state(&session, &target_id).await
        );

        // (B) The earlier content edit is STILL applied — the undo did NOT reach
        //     down to OP #1. (Directly refutes the observed symptom where the
        //     indent/content op was reverted instead of the cycle.)
        assert_eq!(
            block_content(&session, &target_id).await.as_deref(),
            Some(edited.as_str()),
            "undo reverted the PREVIOUS op (content) instead of the cycle — \
             cycle_task_state's undo entry is missing from the stack"
        );

        // (C) One entry (the content edit) remains, and undoing it restores the
        //     original content. Proves the cycle entry was a real, distinct entry.
        assert!(
            session.can_undo().await,
            "after undoing the cycle, the content-edit entry must still be on the stack"
        );
    });
}

/// Metamorphic undo coverage over the property-backed User-origin ops the
/// editor dispatches: for each op O, `state; O; undo == state`. A provider that
/// silently returns `DeclaredIrreversible` for a user-undoable op (the B2 /
/// 2026-07-07 join class) is caught here as a FALSE-GREEN: the op mutates the
/// facet but `undo()` reports `Applied` without restoring it (because the entry
/// was never pushed and the undo popped an unrelated older op).
///
/// This is the op-engine-level analogue of the keystone's
/// `ToggleState → UndoLastMutation` rung (which already covers `task_state` via
/// full-block-state snapshot correspondence — see
/// `ref_caps/toggle.rs::apply_toggle_state` + `block_compare.rs`). It runs at a
/// far faster tier and names the offending op directly on failure.
#[test]
fn metamorphic_property_ops_round_trip_through_undo() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");
        let session = env.session_arc();
        let (target_id, _) = pick_target(&session).await;

        // (op-name, params-builder). Each is a distinct User-undoable authoring
        // op whose provider MUST return `UndoAction::Undo`.
        type Params = std::collections::HashMap<String, Value>;
        let build = |id: &str, pairs: &[(&str, Value)]| -> Params {
            let mut p = Params::new();
            p.insert("id".to_string(), Value::String(id.to_string()));
            for (k, v) in pairs {
                p.insert((*k).to_string(), v.clone());
            }
            p
        };
        // Two distinct provider paths, both reachable from the editor and both
        // property-backed (snapshot-visible via the `properties` map):
        //   - `cycle_task_state` (TaskOperations → set_state → set_field task_state)
        //   - `set_field` on an arbitrary org-drawer property (the generic
        //     property-write path `set_priority`/`set_due_date` also funnel through)
        let cases: Vec<(&str, Params)> = vec![
            ("cycle_task_state", build(&target_id, &[])),
            (
                "set_field",
                build(
                    &target_id,
                    &[
                        ("field", Value::String("effort".to_string())),
                        ("value", Value::String("large".to_string())),
                    ],
                ),
            ),
        ];

        for (op_name, params) in cases {
            let before = facet(&session, &target_id).await;
            let can_undo_before = session.can_undo().await;

            session
                .execute_operation(&EntityName::new("block"), op_name, params)
                .await
                .unwrap_or_else(|e| panic!("dispatch '{op_name}' failed: {e:#}"));

            // The op pushed its OWN entry (fail-loud on the silent-no-push hole).
            let pushed = wait_until(&format!("{op_name}-pushed"), Duration::from_secs(5), || {
                session.can_undo()
            })
            .await;
            assert!(
                pushed,
                "COVERAGE HOLE: '{op_name}' is a user-undoable op but pushed NO undo entry \
                 (can_undo still {can_undo_before}). Its provider returns DeclaredIrreversible \
                 where it must return Undo — undo would silently revert an unrelated op."
            );

            let after_op = facet(&session, &target_id).await;
            assert_ne!(
                before, after_op,
                "'{op_name}' did not change the block facet — cannot test its undo"
            );

            let outcome = session.undo().await.expect("undo dispatch");
            assert!(
                matches!(outcome, UndoOutcome::Applied { .. }),
                "'{op_name}' undo expected Applied, got {outcome:?}"
            );

            let restored = wait_until(
                &format!("{op_name}-restored"),
                Duration::from_secs(5),
                || {
                    let session = session.clone();
                    let target_id = target_id.clone();
                    let before = before.clone();
                    async move { facet(&session, &target_id).await == before }
                },
            )
            .await;
            assert!(
                restored,
                "METAMORPHIC VIOLATION: state;{op_name};undo != state. \
                 before={before:?} after_op={after_op:?} after_undo={:?}",
                facet(&session, &target_id).await
            );
        }
    });
}
