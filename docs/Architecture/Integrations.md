# External Integrations & Frontend

_Part of [Architecture](../Architecture.md)_

## External System Integration

### MCP Apps: Interactive UI Hosting

Holon embraces **[MCP Apps](https://github.com/modelcontextprotocol/ext-apps)** ([SEP-1865](https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/2026-01-26/apps.mdx)), the standard MCP extension that lets servers deliver interactive HTML UIs — charts, forms, dashboards, kanban boards — rendered securely in sandboxed iframes inside any compliant host. Holon acts as an MCP Apps **host**, embedding these UIs directly in its Dioxus web frontend.

#### Why MCP Apps for Holon

Holon's vision demands **custom visualizations** per item type (kanban, burndown charts, calendar views) and **embedded interactive blocks** for third-party items. MCP Apps solves this without Holon writing rendering code per integration:

- **JIRA MCP server** provides a sprint burndown chart → rendered inline in Holon
- **Todoist MCP server** provides a kanban board → embedded in a project page
- **Google Calendar MCP server** provides an interactive week view → displayed in Orient mode
- **Holon's own AI services** expose Watcher dashboards, Integrator confirmation streams, and Guide insights as MCP Apps — available both within Holon and in external chat clients

This gives each integration provider ownership of their visualization while Holon provides the unified data context. The confirmation-driven edge creation stream (see [Vision/AI.md](../../Vision/AI.md) §The Integrator) is a particularly strong fit — an interactive widget where the user confirms or rejects proposed cross-system links at keystroke speed, powered by Holon's local entity graph.

#### Dioxus as the Ideal Host

Because Holon's frontend runs in the browser via **Dioxus** (Rust compiled to WASM), the MCP Apps host role maps directly to native browser capabilities:

| MCP Apps Concept                      | Holon Implementation                                                       |
| ------------------------------------- | -------------------------------------------------------------------------- |
| Sandboxed iframe                      | Native `<iframe>` with `sandbox` attribute, CSP enforced by browser        |
| `postMessage` transport               | `web-sys` bindings to `window.postMessage` + `MessageEvent`                |
| UI resource fetch                     | Browser-native `fetch()` or Holon's HTTP client proxying `ui://` resources |
| Display modes (inline/fullscreen/PiP) | Dioxus layout primitives + CSS                                             |
| Host theming (CSS custom properties)  | Passed through to iframe via `web-sys` DOM access                          |
| CSP enforcement                       | Browser enforces `Content-Security-Policy` on sandboxed iframe origin      |

This is a significant advantage over native (Flutter) frontends where webview sandboxing requires OS-specific APIs. In the browser, the entire MCP Apps security model comes for free.

#### Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│                    HOLON DIOXUS FRONTEND (Browser)               │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │  MCP Apps Host (AppBridge)                                  │  │
│  │  • Renders ui:// resources in sandboxed iframes             │  │
│  │  • Proxies postMessage ↔ MCP JSON-RPC                       │  │
│  │  • Enforces CSP from UIResourceMeta.csp                     │  │
│  │  • Manages iframe lifecycle (init → data → teardown)        │  │
│  └───────────────────────────┬────────────────────────────────┘  │
│                              │                                    │
│  ┌───────────────────────────▼────────────────────────────────┐  │
│  │  Sandboxed iframe                                           │  │
│  │  ┌──────────────────────────────────────────────────────┐  │  │
│  │  │  MCP App (View)                                       │  │  │
│  │  │  • Interactive chart / kanban / form / dashboard     │  │  │
│  │  │  • Calls MCP tools via postMessage → AppBridge        │  │  │
│  │  │  • Adapts to host theme (--color-background, etc.)   │  │  │
│  │  └──────────────────────────────────────────────────────┘  │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                                                  │
│  ┌────────────────────────────────────────────────────────────┐  │
│  │  Holon UI (Dioxus)                                          │  │
│  │  • Outliner blocks, Orient Dashboard, Flow Mode            │  │
│  │  • Embeds MCP App iframes as block-level or fullscreen      │  │
│  │  • Passes tool results (from Holon's cache) to MCP App     │  │
│  └────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────┘
                               │
                               │ WASM ↔ Backend (Rust)
                               ▼
┌──────────────────────────────────────────────────────────────────┐
│                    HOLON BACKEND (Rust)                           │
│                                                                  │
│  ┌────────────────────┐  ┌────────────────────────────────────┐  │
│  │ McpOperationProvider│  │  Unified Turso Cache                │  │
│  │ (tool execution)   │  │  • All third-party data              │  │
│  │                    │  │  • Entity graph + embeddings         │  │
│  └────────┬───────────┘  │  • Operation queue                   │  │
│           │              └────────────────────────────────────┘  │
│           │                                                       │
│  ┌────────▼───────────┐  ┌────────────────────────────────────┐  │
│  │ MCP Server Peers    │  │  AI Services                        │  │
│  │ • Todoist           │  │  • Watcher (monitoring, synthesis)  │  │
│  │ • JIRA              │  │  • Integrator (linking, context)    │  │
│  │ • Calendar          │  │  • Guide (patterns, Shadow Work)    │  │
│  │ • Holon AI (self)   │  └────────────────────────────────────┘  │
│  └────────────────────┘                                           │
└──────────────────────────────────────────────────────────────────┘
```

#### Progressive Enhancement

MCP Apps is designed for graceful degradation. When Holon connects to an MCP server, it negotiates the `io.modelcontextprotocol/ui` extension capability. If the server supports it, tools with `_meta.ui` metadata get interactive iframe rendering; tools without it continue working as text-based operations via the existing `McpOperationProvider`. This is fundamental: **UI is a progressive enhancement, not a requirement**.

#### Security Model

Holon enforces the MCP Apps security model at the browser level:

- **Sandboxed iframes**: All MCP App views run in `<iframe sandbox="allow-scripts">` with no access to Holon's DOM, cookies, or storage
- **CSP enforcement**: Servers declare required origins via `UIResourceMeta.csp`; the browser enforces these at the iframe level. No external connections by default
- **Auditable communication**: All iframe ↔ host communication uses `postMessage` with origin verification; the `AppBridge` validates message structure before forwarding
- **Origin isolation**: `ui://` resources are served from a dedicated suborigin (`ui.holon.app`) to prevent same-origin policy bypass

This aligns with Holon's [privacy-first design](../../Vision/AI.md#3-privacy-first-ai) — the server declares what it needs, the browser enforces the boundary, and Holon's own DOM is never exposed.

#### Use Cases

| Use Case                    | MCP App Source             | Display Mode  | Holon Context                   |
| --------------------------- | -------------------------- | ------------- | ------------------------------- |
| Sprint burndown chart       | JIRA MCP server            | Inline block  | Project page "Sprint 42"        |
| Kanban board                | Todoist MCP server         | Fullscreen    | Project page "Website Redesign" |
| Week calendar view          | Google Calendar MCP server | Inline panel  | Orient Dashboard                |
| Confirmation stream         | Holon AI MCP server (self) | Inline widget | Orient mode                     |
| Capacity analysis chart     | Holon AI MCP server (self) | Fullscreen    | Watcher Dashboard               |
| Shadow Work prompt          | Holon AI MCP server (self) | Inline widget | Flow mode (stuck task)          |
| Cross-system search results | Holon AI MCP server (self) | Inline panel  | Global search                   |

#### Spec Reference

- [SEP-1865: MCP Apps](https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/2026-01-26/apps.mdx)
- [MCP Apps SDK](https://github.com/modelcontextprotocol/ext-apps) — `@modelcontextprotocol/ext-apps`
- [Quickstart Guide](https://apps.extensions.modelcontextprotocol.io/api/documents/Quickstart.html)

---

### Integration Pattern

Each external system provides:

1. **SyncProvider** - Fetches data from external API
2. **DataSource** - Read access to cached data
3. **OperationProvider** - Routes operations to external API

```rust
// Todoist example
TodoistSyncProvider
  → Incremental sync with sync tokens
  → HTTP requests to Todoist REST API

TodoistTaskDataSource
  → Implements DataSource<TodoistTask>
  → Reads from QueryableCache

TodoistOperationProvider
  → Routes set_field() to Todoist API
  → Returns inverse operation for undo
```

### Adding a New External System

1. Define entity types implementing `IntoEntity` + `TryFromEntity`
2. Implement `DataSource<T>` for read access
3. Implement domain traits (`TaskOperations`, etc.)
4. Create `SyncProvider` for data synchronization
5. Register in DI container

### MCP Client Integration (holon-mcp-client)

External systems that expose an MCP server can be integrated without writing Rust code per operation. `holon-mcp-client` connects to any MCP server over Streamable HTTP, reads its tool schemas at runtime, and converts them into `OperationDescriptor`s that plug into Holon's existing `OperationDispatcher`.

**Location**: `crates/holon-mcp-client/`

#### Architecture

```
MCP Server (e.g. ai.todoist.net/mcp)
       │
       │  list_tools() → JSON Schema per tool
       ▼
┌─────────────────────────────┐     ┌──────────────────────────┐
│  McpOperationProvider       │◄────│  YAML Sidecar            │
│  • descriptors (cached)     │     │  • entity mapping        │
│  • tool_name_map            │     │  • affected_fields       │
│  • peer (rmcp connection)   │     │  • triggered_by          │
│  • _connection (keep-alive) │     │  • preconditions (Rhai)  │
└──────────┬──────────────────┘     │  • param_overrides       │
           │                        └──────────────────────────┘
           │  implements OperationProvider
           ▼
    OperationDispatcher (aggregates all providers)
```

#### Components

| Component              | File                    | Purpose                                                                                                                                                          |
| ---------------------- | ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `McpOperationProvider` | `mcp_provider.rs`       | Connects to MCP server, caches `OperationDescriptor`s from tool schemas, executes tools via `call_tool`. Holds `McpRunningService` to keep the connection alive. |
| `McpSidecar`           | `mcp_sidecar.rs`        | YAML config that patches UI affordances onto MCP tools: entity mapping, `affected_fields`, `triggered_by`, `precondition` (Rhai), `param_overrides`.             |
| `RhaiPrecondition`     | `mcp_sidecar.rs`        | Parse-don't-validate wrapper: Rhai expressions are validated at YAML deserialization time. Invalid syntax fails immediately, not at operation execution.         |
| `mcp_schema_mapping`   | `mcp_schema_mapping.rs` | Converts JSON Schema types to `TypeHint` (String, Bool, Number, OneOf, EntityId via overrides). Walks `inputSchema.properties` to build `Vec<OperationParam>`.   |
| `connect_mcp()`        | `mcp_provider.rs`       | Establishes Streamable HTTP connection to an MCP server, returns `Peer<RoleClient>` + `McpRunningService`.                                                       |

#### YAML Sidecar

MCP tool schemas carry parameter types and descriptions but lack UI-specific metadata. The YAML sidecar fills this gap:

```yaml
entities:
  todoist_tasks:
    short_name: task
    id_column: id
  todoist_projects:
    short_name: project
    id_column: id

tools:
  complete-tasks:
    entity: todoist_tasks
    affected_fields: [completed]
    triggered_by:
      - from: completed
        provides: [ids]
    precondition: "completed == false" # validated as Rhai at load time
  update-tasks:
    entity: todoist_tasks
    affected_fields: [content, description, priority, dueString, labels]
  add-tasks:
    entity: todoist_tasks
    display_name: Create Task
```

Tools without sidecar entries still appear as operations, but with no gesture bindings (affected_fields, triggered_by, preconditions).

#### Tool Name Normalization

MCP tools use kebab-case (`complete-tasks`), Holon operations use snake_case (`complete_tasks`). `McpOperationProvider` maintains a `tool_name_map` to translate between the two.

#### DI Registration (Todoist Example)

`McpOperationProvider` coexists with existing hand-written providers. In `holon-todoist/src/di.rs`:

```rust
// Existing providers (unchanged):
// - TodoistSyncProvider → dyn SyncableProvider + dyn OperationProvider ("todoist.sync")
// - TodoistTaskOperations → dyn OperationProvider (set_field, indent, move_block, etc.)
// - TodoistProjectDataSource → dyn OperationProvider (move_block for projects)

// New MCP provider (additive):
// - McpOperationProvider → dyn OperationProvider (complete_tasks, update_tasks, add_tasks, ...)
//   Wrapped with OperationWrapper for automatic post-operation sync
```

The `TodoistConfig.mcp_server_uri` field controls whether the MCP provider is registered. When set, `McpOperationProvider::connect()` runs inside a `block_on` in the DI factory (safe because factories execute on the main tokio runtime). The sidecar YAML is bundled at compile time via `include_str!`.

#### Reuse Across Integrations

`holon-mcp-client` is integration-agnostic. To add MCP-backed operations for a new system:

1. Create a YAML sidecar with entity mappings and tool annotations
2. Register `McpOperationProvider` in your integration's DI module with the appropriate MCP server URI
3. Optionally wrap with `OperationWrapper` for post-operation sync

## Frontend Architecture

Holon's primary frontend is a **Dioxus** web application — Rust compiled to WASM, running entirely in the browser. This eliminates the FFI bridge entirely: frontend and backend share the same Rust types, and communication uses direct async function calls rather than serialized RPC.

### Dioxus Web Frontend

```rust
// Inversion of Control: frontend asks for what to render, backend resolves everything.
// No FFI — direct async calls within the same WASM binary (or to a backend server).

async fn get_root_block_id() -> Result<String>;

async fn render_entity(
    block_id: String,
    preferred_variant: Option<String>,
    is_root: bool,
) -> Result<WatchHandle>;  // returns a Stream<UiEvent>

// Operations: frontend dispatches user actions
async fn execute_operation(
    entity: String,
    op: String,
    params: HashMap<String, Value>,
) -> Result<Option<String>>;
```

The frontend never sends queries — it only sends block IDs and receives render instructions. In the browser context, the backend may run in the same WASM thread (for local-only mode) or connect to a remote Holon backend via WebSocket/HTTP.

### Reactive Updates

Frontends subscribe to change streams via Dioxus signals:

```rust
use dioxus::prelude::*;

fn BlockView(block_id: String) -> Element {
    let mut block_data = use_signal(|| None);

    use_effect(move || {
        let mut stream = watch_changes(block_id.clone());
        // Update signal whenever backend pushes a change
        spawn(async move {
            while let Some(change) = stream.next().await {
                block_data.set(Some(change.data));
            }
        });
    });

    // Dioxus auto-rerenders when signal changes
    rsx! { div { "{block_data.read()}" } }
}
```

No explicit refresh calls — UI state derives from the change stream, and Dioxus's fine-grained reactivity handles re-rendering.

### MCP Apps Rendering in Dioxus

The MCP Apps host component renders sandboxed iframes through Dioxus's native `iframe` element support:

```rust
fn McpAppView(server: String, tool_name: String, resource_uri: String) -> Element {
    let app_bridge = use_coroutine(|mut rx| async move {
        let bridge = AppBridge::new(&server, &tool_name, &resource_uri).await;
        while let Some(msg) = rx.next().await {
            bridge.handle_message(msg).await;
        }
    });

    rsx! {
        iframe {
            src: "{resource_uri}",
            sandbox: "allow-scripts",
            onload: move |_| app_bridge.send(AppBridgeMsg::Initialize),
        }
    }
}
```

Because Dioxus runs in the browser, `web-sys` provides direct access to `postMessage`, `MessageEvent`, and iframe lifecycle hooks — no platform abstraction layer needed.

## Dependency Injection

Using `ferrous-di` for service composition:

```rust
pub async fn create_backend_engine<F>(
    db_path: PathBuf,
    setup_fn: F,
) -> Result<Arc<BackendEngine>>

// Registers:
// - TursoBackend
// - OperationDispatcher
// - TransformPipeline
// - Provider modules (Todoist, OrgMode, etc.)
```
