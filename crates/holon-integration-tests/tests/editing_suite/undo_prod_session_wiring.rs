//! Env-gap closure for BugFunnel row 33 ("undo dead in prod GPUI").
//!
//! The four headless undo tests (`crates/holon/tests/undo_*.rs`) construct a
//! provider-level `BackendEngine` directly. None of them drive the PROD
//! session-construction path — the DI-wired `FrontendSession` the GPUI app
//! actually dispatches through (`holon-app` wiring → the DI `BackendEngine`
//! singleton with `enable_undo_persistence`). This test closes that gap: it
//! boots the real DI container via `TestEnvironment::start_app` (the same
//! wiring the desktop app resolves), dispatches a User-origin `set_field` the
//! way the editor's `dispatch_intent` does
//! (`FrontendSession::execute_operation` stamps `OpOrigin::User`), and asserts:
//!
//!   1. After the edit, `session.can_undo()` is TRUE — the prod session wires a
//!      populated undo stack (the row-33 "gpui session doesn't wire the stack"
//!      hypothesis).
//!   2. A `HolonService` built over the SAME DI `BackendEngine` — exactly what
//!      the embedded MCP's `service()` does (`HolonService::new_with_origin`,
//!      `OpOrigin::Agent`) — ALSO reports `can_undo() == true`. This refutes
//!      the row-33 "MCP reads a different stack" hypothesis: editor and MCP
//!      share ONE stack because they share ONE DI `BackendEngine`.
//!   3. `session.undo()` returns `Applied` and the block content is reverted to
//!      its pre-edit value.
//!
//! It also documents the correctly-behaving other half of the row-33 evidence:
//! an Agent-origin `set_field` (an MCP tool call) does NOT push an undo entry
//! (`OpOrigin::Agent` is not `is_user()`; agent actions revert through the
//! supervision surface, ADR 0024 C2a). So "agent set_field → can_undo false" is
//! by-design, not the bug — asserted here so a regression that starts
//! journaling agent ops onto the human stack is caught.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::api::HolonService;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::UndoOutcome;
use holon_api::Value;
use holon_integration_tests::test_environment::TestEnvironment;

/// Poll `f` until it returns `true` or the deadline elapses. can_undo /
/// projection settle is async across the CDC boundary; a bounded poll avoids a
/// flaky fixed sleep.
async fn wait_until<F, Fut>(label: &str, timeout: Duration, mut f: F) -> bool
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    eprintln!("wait_until('{label}') timed out after {timeout:?}");
    false
}

/// Read one block's content from the session's block-query snapshot (the same
/// projection the UI renders from), or `None` if absent.
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

/// Pick a stable, editable seeded block to target: a `block:` content block
/// with non-empty content that is not a structural `::src::` / `::render::`
/// node. Returns (id, original_content).
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
fn prod_session_wires_undo_and_mcp_shares_the_same_stack() {
    let runtime = Arc::new(tokio::runtime::Runtime::new().expect("tokio runtime"));
    runtime.clone().block_on(async move {
        let env = TestEnvironment::new(runtime.clone()).unwrap();
        env.start_app(true).await.expect("start_app");

        let session = env.session_arc();
        let engine = env.engine().clone();

        // Fresh boot: nothing to undo.
        assert!(
            !session.can_undo().await,
            "fresh prod session must start with an empty undo stack"
        );

        let (target_id, original) = pick_target(&session).await;
        let edited = format!("{original} edited-by-repro");

        // ── Editor typing analog: a User-origin content set_field, exactly the
        //    shape `ReactiveEngine::dispatch_intent` sends through
        //    `FrontendSession::execute_operation` (which stamps OpOrigin::User).
        // Faithful to `editor_view.rs`'s `InputEvent::Change` SqlOnly dispatch:
        // it stamps a monotonic `write_seq` ordering token alongside `content`
        // (row 40 echo-suppression). Included here so the test proves the
        // write_seq column does NOT change the undo classification (the op still
        // returns `UndoAction::Undo` and pushes an entry).
        let seq = holon_api::write_seq::next();
        let mut params = std::collections::HashMap::new();
        params.insert("id".to_string(), Value::String(target_id.clone()));
        params.insert("field".to_string(), Value::String("content".to_string()));
        params.insert("value".to_string(), Value::String(edited.clone()));
        params.insert("write_seq".to_string(), Value::Integer(seq.get()));
        session
            .execute_operation(&EntityName::new("block"), "set_field", params)
            .await
            .expect("user set_field dispatch");

        // (1) The prod session wired a populated undo stack.
        let can_undo_session = wait_until("session.can_undo", Duration::from_secs(5), || {
            session.can_undo()
        })
        .await;
        assert!(
            can_undo_session,
            "PROD SESSION GAP: after a User-origin edit the session's undo stack is still empty \
             (can_undo=false) — the editor→FrontendSession→BackendEngine undo push is not wired"
        );

        // (2) The embedded MCP reads the SAME stack. `HolonService::new_with_origin`
        //     over the DI BackendEngine singleton is precisely what
        //     `HolonMcpServer::service()` builds. If the MCP saw a different
        //     engine, this would be false while (1) is true.
        let mcp_service = HolonService::new_with_origin(
            engine.clone(),
            OpOrigin::Agent {
                session_id: "mcp-session:repro".to_string(),
                tool_call_id: "tool-call:repro".to_string(),
            },
        );
        assert!(
            mcp_service.can_undo().await,
            "MCP-STACK GAP: the editor's undo entry is invisible to a HolonService over the same \
             DI BackendEngine — editor and MCP are not sharing one undo stack"
        );

        // (3) Undo reverts the content.
        let outcome = session.undo().await.expect("undo dispatch");
        assert!(
            matches!(outcome, UndoOutcome::Applied { .. }),
            "expected UndoOutcome::Applied, got {outcome:?}"
        );
        let reverted = wait_until("content-reverted", Duration::from_secs(5), || {
            let session = session.clone();
            let target_id = target_id.clone();
            let original = original.clone();
            async move {
                block_content(&session, &target_id).await.as_deref() == Some(original.as_str())
            }
        })
        .await;
        assert!(
            reverted,
            "undo did not revert '{target_id}' to its original content '{original}' (still: {:?})",
            block_content(&session, &target_id).await
        );

        // After the single edit is undone, the stack is empty again.
        assert!(
            !session.can_undo().await,
            "stack should be empty after undoing the only user edit"
        );

        // ── By-design counter-check: an Agent-origin edit (MCP tool call) must
        //    NOT push a human undo entry (OpOrigin::Agent is not is_user()).
        //    This is the correctly-behaving half of the row-33 evidence.
        let mut agent_params = std::collections::HashMap::new();
        agent_params.insert("id".to_string(), Value::String(target_id.clone()));
        agent_params.insert("field".to_string(), Value::String("content".to_string()));
        agent_params.insert(
            "value".to_string(),
            Value::String(format!("{original} edited-by-agent")),
        );
        engine
            .execute_operation(
                &EntityName::new("block"),
                "set_field",
                agent_params
                    .into_iter()
                    .map(|(k, v)| (Arc::from(k.as_str()), v))
                    .collect(),
                OpOrigin::Agent {
                    session_id: "mcp-session:repro".to_string(),
                    tool_call_id: "tool-call:repro-2".to_string(),
                },
            )
            .await
            .expect("agent set_field dispatch");
        assert!(
            !session.can_undo().await,
            "REGRESSION: an Agent-origin edit pushed onto the human undo stack — agent actions \
             must revert through the supervision surface, not cmd+z (ADR 0024 C2a)"
        );
    });
}
