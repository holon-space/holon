use std::collections::HashMap;
use std::sync::Arc;

use holon_api::Value;
use holon_frontend::FrontendSession;

/// Dioxus-specific: spawns on the ambient tokio runtime (no handle needed).
pub fn dispatch_operation(
    session: &Arc<FrontendSession>,
    entity_name: String,
    op_name: String,
    params: HashMap<String, Value>,
) {
    let session = Arc::clone(session);
    tokio::spawn(async move {
        if let Err(e) = session
            .execute_operation(&entity_name, &op_name, params)
            .await
        {
            tracing::error!("Operation {entity_name}.{op_name} failed: {e}");
        }
    });
}
