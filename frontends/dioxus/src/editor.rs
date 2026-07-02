//! Native (webview) editable cell for `editable_text`.
//!
//! The wasm dioxus-web frontend preserves the cursor by hand over a
//! `contenteditable` div + `web_sys::Selection`. On desktop the webview keeps
//! DOM/selection state across dioxus diffing, so we use a plain `<input>` that
//! commits on blur (`onchange`) — the same shape the original desktop frontend
//! used — and dispatch `block.update` through the injected session.

use std::collections::HashMap;
use std::sync::Arc;

use dioxus::prelude::*;
use holon_api::{EntityName, Value};
use holon_frontend::FrontendSession;

#[component]
pub fn EditorCell(entity_id: String, content: String) -> Element {
    let session: Arc<FrontendSession> = use_context();
    let rt: tokio::runtime::Handle = use_context();

    rsx! {
        input {
            r#type: "text",
            value: "{content}",
            style: "width: 100%; background: transparent; border: none; outline: none; \
                    color: var(--text-primary); font-family: inherit; font-size: inherit; padding: 0;",
            // Keystrokes inside the editor belong to the webview's native
            // text editing (incl. Cmd+Z text undo) — never to the root
            // div's global data undo/redo shortcuts.
            onkeydown: move |evt| evt.stop_propagation(),
            onchange: move |evt| {
                let new_content = evt.value();
                let session = session.clone();
                let id = entity_id.clone();
                rt.spawn(async move {
                    let mut params = HashMap::new();
                    params.insert("id".to_string(), Value::String(id));
                    params.insert("content".to_string(), Value::String(new_content));
                    if let Err(e) = session
                        .execute_operation(&EntityName::new("block"), "update", params)
                        .await
                    {
                        tracing::error!("[editable_text] block.update failed: {e}");
                    }
                });
            },
        }
    }
}
