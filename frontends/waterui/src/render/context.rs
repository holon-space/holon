use std::collections::HashMap;
use std::sync::Arc;

use holon_api::render_types::OperationWiring;
use holon_api::Value;
use holon_frontend::FrontendSession;

pub struct RenderContext {
    pub data_rows: Vec<HashMap<String, Value>>,
    pub operations: Vec<OperationWiring>,
    pub session: Arc<FrontendSession>,
    pub runtime_handle: tokio::runtime::Handle,
    pub depth: usize,
    pub query_depth: usize,
}

impl RenderContext {
    pub fn new(session: Arc<FrontendSession>, runtime_handle: tokio::runtime::Handle) -> Self {
        Self {
            data_rows: Vec::new(),
            operations: Vec::new(),
            session,
            runtime_handle,
            depth: 0,
            query_depth: 0,
        }
    }

    pub fn row(&self) -> &HashMap<String, Value> {
        static EMPTY: std::sync::LazyLock<HashMap<String, Value>> =
            std::sync::LazyLock::new(HashMap::new);
        self.data_rows.first().unwrap_or(&EMPTY)
    }

    fn child(&self) -> Self {
        Self {
            data_rows: self.data_rows.clone(),
            operations: self.operations.clone(),
            session: Arc::clone(&self.session),
            runtime_handle: self.runtime_handle.clone(),
            depth: self.depth,
            query_depth: self.query_depth,
        }
    }

    pub fn with_row(&self, row: HashMap<String, Value>) -> Self {
        Self {
            data_rows: vec![row],
            ..self.child()
        }
    }

    pub fn with_data_rows(&self, data_rows: Vec<HashMap<String, Value>>) -> Self {
        Self {
            data_rows,
            ..self.child()
        }
    }

    pub fn deeper_query(&self) -> Self {
        Self {
            query_depth: self.query_depth + 1,
            ..self.child()
        }
    }
}
