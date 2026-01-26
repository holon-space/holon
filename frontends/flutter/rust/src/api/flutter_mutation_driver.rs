//! Flutter-specific MutationDriver that delegates UI mutations to Dart via DartFnFuture.
//!
//! The Dart callback receives (entity, op, params_json) and uses WidgetTester
//! to drive the mutation through the Flutter UI. Phase 1: Dart calls executeOperation
//! directly. Phase 2: actual WidgetTester interactions (tap, enterText, drag).

use anyhow::Result;
use flutter_rust_bridge::DartFnFuture;
use holon_api::Value;
use holon_integration_tests::MutationDriver;
use std::collections::HashMap;
use std::sync::Arc;

/// Callback signature: (entity, op, params_json) → Future<()>
///
/// The Dart side receives the mutation as JSON-serialized params and dispatches it
/// through the Flutter UI (or calls executeOperation directly in Phase 1).
pub type ApplyMutationCallback =
    Arc<dyn Fn(String, String, String) -> DartFnFuture<()> + Send + Sync>;

pub struct FlutterMutationDriver {
    apply_mutation_cb: ApplyMutationCallback,
}

impl FlutterMutationDriver {
    pub fn new(apply_mutation_cb: ApplyMutationCallback) -> Self {
        Self { apply_mutation_cb }
    }
}

#[async_trait::async_trait]
impl MutationDriver for FlutterMutationDriver {
    async fn apply_ui_mutation(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()> {
        let params_json = serde_json::to_string(&params)?;
        (self.apply_mutation_cb)(entity.to_string(), op.to_string(), params_json).await;
        Ok(())
    }
}
