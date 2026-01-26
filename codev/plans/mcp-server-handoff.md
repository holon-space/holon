# MCP Server Implementation Handoff

## Task
Create an MCP server at `frontends/mcp` that exposes the holon BackendEngine API via MCP protocol.

## SDK
Use `rmcp` (official Rust MCP SDK): https://github.com/modelcontextprotocol/rust-sdk

---

## Files to Create

### 1. `frontends/mcp/Cargo.toml`

```toml
[package]
name = "holon-mcp"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "holon-mcp"
path = "src/main.rs"

[dependencies]
rmcp = { git = "https://github.com/modelcontextprotocol/rust-sdk", features = ["server", "macros"] }
tokio = { workspace = true, features = ["full"] }
tokio-stream.workspace = true
serde.workspace = true
serde_json.workspace = true
schemars = "0.8"
anyhow.workspace = true
async-trait.workspace = true
holon.workspace = true
holon-api.workspace = true
holon-prql-render.workspace = true
ferrous-di = { workspace = true, features = ["async"] }
uuid.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
```

### 2. Update workspace `Cargo.toml`

Add `"frontends/mcp"` to the `members` array.

### 3. `frontends/mcp/src/main.rs`

Entry point using stdio transport:

```rust
use anyhow::Result;
use rmcp::{ServiceExt, transport::stdio};
use std::path::PathBuf;
use tracing_subscriber::{self, EnvFilter};

mod server;
mod tools;
mod resources;
mod types;

use server::HolonMcpServer;

#[tokio::main]
async fn main() -> Result<()> {
    // Log to stderr (stdout is for MCP protocol)
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .with_writer(std::io::stderr)
        .init();

    // Parse optional db_path argument
    let db_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(":memory:"));

    // Create backend engine using DI (same pattern as TUI)
    let engine = holon::di::create_backend_engine(db_path, |_services| Ok(())).await?;

    // Create and run MCP server
    let server = HolonMcpServer::new(engine);
    let running = server.serve(stdio()).await?;
    running.waiting().await?;

    Ok(())
}
```

### 4. `frontends/mcp/src/lib.rs`

```rust
pub mod server;
pub mod tools;
pub mod resources;
pub mod types;
```

### 5. `frontends/mcp/src/types.rs`

Parameter and result types with JSON Schema support:

```rust
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CreateTableParams {
    pub table_name: String,
    pub columns: Vec<ColumnDef>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ColumnDef {
    pub name: String,
    pub sql_type: String,  // TEXT, INTEGER, BOOLEAN, etc.
    #[serde(default)]
    pub primary_key: bool,
    pub default: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct InsertDataParams {
    pub table_name: String,
    pub rows: Vec<HashMap<String, serde_json::Value>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ExecutePrqlParams {
    pub prql: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ExecuteSqlParams {
    pub sql: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ExecuteOperationParams {
    pub entity_name: String,
    pub operation: String,
    pub params: HashMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct WatchQueryParams {
    pub prql: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct WatchHandle {
    pub watch_id: String,
    pub initial_data: Vec<HashMap<String, serde_json::Value>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct QueryResult {
    pub rows: Vec<HashMap<String, serde_json::Value>>,
    pub row_count: usize,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct RowChangeJson {
    pub change_type: String,  // "Created", "Updated", "Deleted"
    pub entity_id: Option<String>,
    pub data: Option<HashMap<String, serde_json::Value>>,
}
```

### 6. `frontends/mcp/src/server.rs`

Main server struct and ServerHandler implementation:

```rust
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use holon::api::backend_engine::BackendEngine;
use rmcp::{ServerHandler, ServerInfo, model::*};

use crate::types::RowChangeJson;

pub struct WatchState {
    pub pending_changes: Vec<RowChangeJson>,
    pub _task_handle: JoinHandle<()>,
}

pub struct HolonMcpServer {
    pub engine: Arc<BackendEngine>,
    pub watches: Arc<Mutex<HashMap<String, WatchState>>>,
}

impl HolonMcpServer {
    pub fn new(engine: Arc<BackendEngine>) -> Self {
        Self {
            engine,
            watches: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl ServerHandler for HolonMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            name: "holon-mcp".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            instructions: Some("Holon backend engine MCP server for automated testing".into()),
            ..Default::default()
        }
    }

    // Implement list_tools, call_tool, list_resources, read_resource
    // See rmcp examples for patterns
}
```

### 7. `frontends/mcp/src/tools.rs`

Tool implementations. Use the `#[tool]` macro from rmcp:

```rust
use rmcp::{tool, model::*};
use crate::server::HolonMcpServer;
use crate::types::*;

#[tool(tool_box)]
impl HolonMcpServer {
    #[tool(description = "Create a table with specified schema")]
    async fn create_table(&self, #[tool(aggr)] params: CreateTableParams) -> Result<CallToolResult, rmcp::Error> {
        // Build CREATE TABLE SQL from params.columns
        // Execute via self.engine.execute_query()
        todo!()
    }

    #[tool(description = "Insert rows into a table")]
    async fn insert_data(&self, #[tool(aggr)] params: InsertDataParams) -> Result<CallToolResult, rmcp::Error> {
        // Build INSERT SQL from params.rows
        // Execute via self.engine.execute_query()
        todo!()
    }

    #[tool(description = "Execute a PRQL query and return results")]
    async fn execute_prql(&self, #[tool(aggr)] params: ExecutePrqlParams) -> Result<CallToolResult, rmcp::Error> {
        // Call self.engine.compile_query() then execute_query()
        todo!()
    }

    #[tool(description = "Start watching a query for CDC changes")]
    async fn watch_query(&self, #[tool(aggr)] params: WatchQueryParams) -> Result<CallToolResult, rmcp::Error> {
        // Call self.engine.query_and_watch()
        // Spawn background task to collect changes
        // Store in self.watches with uuid
        // Return WatchHandle
        todo!()
    }

    #[tool(description = "Poll for accumulated CDC changes")]
    async fn poll_changes(&self, watch_id: String) -> Result<CallToolResult, rmcp::Error> {
        // Drain pending_changes from self.watches[watch_id]
        todo!()
    }

    #[tool(description = "Execute an operation on an entity")]
    async fn execute_operation(&self, #[tool(aggr)] params: ExecuteOperationParams) -> Result<CallToolResult, rmcp::Error> {
        // Convert params to StorageEntity
        // Call self.engine.execute_operation()
        todo!()
    }

    #[tool(description = "Undo the last operation")]
    async fn undo(&self) -> Result<CallToolResult, rmcp::Error> {
        let result = self.engine.undo().await;
        // Return success/failure
        todo!()
    }

    #[tool(description = "Redo the last undone operation")]
    async fn redo(&self) -> Result<CallToolResult, rmcp::Error> {
        let result = self.engine.redo().await;
        todo!()
    }
}
```

### 8. `frontends/mcp/src/resources.rs`

Resource handlers for operations listing:

```rust
use rmcp::model::*;
use crate::server::HolonMcpServer;

impl HolonMcpServer {
    pub async fn list_resources_impl(&self) -> ListResourcesResult {
        ListResourcesResult {
            resources: vec![
                Resource {
                    uri: "holon://operations".into(),
                    name: "Available Operations".into(),
                    description: Some("List all available entity operations".into()),
                    mime_type: Some("application/json".into()),
                    ..Default::default()
                },
            ],
            next_cursor: None,
        }
    }

    pub async fn read_resource_impl(&self, uri: &str) -> Result<ReadResourceResult, rmcp::Error> {
        if uri.starts_with("holon://operations/") {
            let entity = uri.strip_prefix("holon://operations/").unwrap();
            let ops = self.engine.available_operations(entity).await;
            // Serialize ops to JSON
            todo!()
        }
        Err(rmcp::Error::resource_not_found(uri))
    }
}
```

---

## Reference Code

### BackendEngine API (from `crates/holon/src/api/backend_engine.rs`)

```rust
// Key methods to expose:
engine.compile_query(prql: String) -> Result<(String, RenderSpec)>
engine.execute_query(sql: String, params: HashMap<String, Value>) -> Result<Vec<HashMap<String, Value>>>
engine.query_and_watch(prql: String, params: HashMap) -> Result<(RenderSpec, Vec<HashMap>, RowChangeStream)>
engine.execute_operation(entity_name: &str, op_name: &str, params: StorageEntity) -> Result<()>
engine.available_operations(entity_name: &str) -> Vec<OperationDescriptor>
engine.undo() -> Result<bool>
engine.redo() -> Result<bool>
engine.can_undo() -> bool
engine.can_redo() -> bool
```

### DI Pattern (from `frontends/tui/src/launcher.rs`)

```rust
let engine = holon::di::create_backend_engine(db_path, |services| {
    // Optional: register additional modules
    Ok(())
}).await?;
```

### CDC Watch Pattern (from `crates/holon/tests/e2e_backend_engine_test.rs`)

```rust
let (_render_spec, initial_data, stream) = ctx.query_and_watch(prql, HashMap::new()).await?;

// Collect changes from stream
use tokio_stream::StreamExt;
while let Some(batch) = stream.next().await {
    for change in batch.inner.items {
        // Process RowChange
    }
}
```

---

## Tools Summary

| Tool | Parameters | Returns |
|------|------------|---------|
| `create_table` | `{table_name, columns: [{name, sql_type, primary_key?, default?}]}` | success message |
| `insert_data` | `{table_name, rows: [{}]}` | row count |
| `drop_table` | `{table_name}` | success message |
| `execute_prql` | `{prql, params?}` | `{rows, row_count}` |
| `execute_sql` | `{sql, params?}` | `{rows, row_count}` |
| `watch_query` | `{prql, params?}` | `{watch_id, initial_data}` |
| `poll_changes` | `{watch_id}` | `[{change_type, entity_id, data}]` |
| `stop_watch` | `{watch_id}` | success message |
| `execute_operation` | `{entity_name, operation, params}` | success message |
| `list_operations` | `{entity_name}` | `[OperationDescriptor]` |
| `undo` | - | `{success, message}` |
| `redo` | - | `{success, message}` |
| `can_undo` | - | `{available: bool}` |
| `can_redo` | - | `{available: bool}` |

---

## Build & Test

```bash
# Build
cargo build -p holon-mcp

# Test with MCP Inspector
npx @modelcontextprotocol/inspector ./target/debug/holon-mcp

# Test with in-memory database
./target/debug/holon-mcp :memory:
```
