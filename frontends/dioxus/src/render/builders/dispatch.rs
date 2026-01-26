//! Shared click→operation dispatch for interactive builders.
//!
//! The desktop frontend runs the engine in-process, so a builder dispatches an
//! `OperationIntent` by calling `FrontendSession::execute_operation` on the
//! injected tokio `Handle`. This mirrors `gpui`'s
//! `BuilderServices::dispatch_intent` — the single place display builders
//! (`selectable`, `block_operations`, …) turn a click into a write.

use std::sync::Arc;

use dioxus::prelude::Modifiers;
use holon_api::ClickModifiers;
use holon_frontend::FrontendSession;
use holon_frontend::operations::OperationIntent;

/// Fire-and-forget dispatch of an `OperationIntent` onto the tokio runtime.
///
/// Errors are logged loudly (never swallowed silently) — the UI keeps running
/// but the failure is visible in the log, matching the project's
/// fail-loud-never-fake policy.
pub(crate) fn dispatch_intent(
    rt: &tokio::runtime::Handle,
    session: &Arc<FrontendSession>,
    intent: OperationIntent,
) {
    let session = session.clone();
    rt.spawn(async move {
        if let Err(e) = session
            .execute_operation(&intent.entity_name, &intent.op_name, intent.params)
            .await
        {
            tracing::error!(
                "[dispatch] {}.{} failed: {e}",
                intent.entity_name,
                intent.op_name
            );
        }
    });
}

/// Translate a dioxus/keyboard-types modifier set into Holon's
/// `ClickModifiers` (cmd == platform META key).
pub(crate) fn click_modifiers(m: Modifiers) -> ClickModifiers {
    ClickModifiers {
        shift: m.contains(Modifiers::SHIFT),
        alt: m.contains(Modifiers::ALT),
        cmd: m.contains(Modifiers::META),
        ctrl: m.contains(Modifiers::CONTROL),
    }
}
