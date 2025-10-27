//! MutationDriver trait — abstracts how UI mutations are dispatched.
//!
//! `DirectMutationDriver` calls `BackendEngine::execute_operation` directly (used by backend PBT).
//! `FlutterMutationDriver` (in the Flutter crate) calls DartFnFuture callbacks that drive WidgetTester.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::sync::Arc;

use holon::api::backend_engine::BackendEngine;
use holon_api::Value;

/// How UI mutations are dispatched to the system under test.
///
/// Backend tests use `DirectMutationDriver` (calls `execute_op` directly).
/// Flutter tests provide a `FlutterMutationDriver` that calls Dart callbacks
/// which drive WidgetTester interactions.
#[async_trait::async_trait]
pub trait MutationDriver: Send + Sync {
    async fn apply_ui_mutation(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()>;
}

/// Dispatches mutations directly via `BackendEngine::execute_operation`.
pub struct DirectMutationDriver {
    engine: Arc<BackendEngine>,
}

impl DirectMutationDriver {
    pub fn new(engine: Arc<BackendEngine>) -> Self {
        Self { engine }
    }
}

#[async_trait::async_trait]
impl MutationDriver for DirectMutationDriver {
    async fn apply_ui_mutation(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()> {
        self.engine
            .execute_operation(entity, op, params)
            .await
            .map(|_| ())
            .context(format!("execute_operation({entity}, {op}) failed"))
    }
}
