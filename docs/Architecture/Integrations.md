# External Integrations & Frontend

_Part of [Architecture](../Architecture.md). Reconciled with code on 2026-09-03._

## External System Integration

### MCP Apps: Interactive UI Hosting

> **Status: target architecture, premised on a parked frontend.** No `AppBridge` struct exists anywhere in `frontends/`. The section below describes the intended design for the Dioxus web frontend, which is parked (vault `Multi-Frontend Strategy.org`) — GPUI is the shipping primary frontend. This design has no current host to land in.

Holon embraces **[MCP Apps](https://github.com/modelcontextprotocol/ext-apps)** ([SEP-1865](https://github.com/modelcontextprotocol/ext-apps/blob/main/specification/2026-01-26/apps.mdx)), the standard MCP extension that lets servers deliver interactive HTML UIs — charts, forms, dashboards, kanban boards — rendered securely in sandboxed iframes inside any compliant host. Holon acts as an MCP Apps **host**, embedding these UIs in its Dioxus web frontend.

#### Why MCP Apps for Holon

Holon's vision demands **custom visualizations** per item type (kanban, burndown charts, calendar views) and **embedded interactive blocks** for third-party items. MCP Apps solves this without Holon writing rendering code per integration:

- **JIRA MCP server** provides a sprint burndown chart → rendered inline in Holon
- **Todoist MCP server** provides a kanban board → embedded in a project page
- **Google Calendar MCP server** provides an interactive week view → displayed in Orient mode
- **Holon's own AI services** expose Watcher dashboards, Integrator confirmation streams, and Guide insights as MCP Apps — available both within Holon and in external chat clients

This gives each integration provider ownership of their visualization while Holon provides the unified data context. The confirmation-driven edge creation stream (see [Vision/AI.md](../Vision/AI.md) §The Integrator) is a particularly strong fit — an interactive widget where the user confirms or rejects proposed cross-system links at keystroke speed, powered by Holon's local entity graph.

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

This aligns with Holon's [privacy-first design](../Vision/AI.md#3-privacy-first-ai) — the server declares what it needs, the browser enforces the boundary, and Holon's own DOM is never exposed.

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

External MCP-based integrations are declared entirely in YAML sidecars; no
per-integration Rust code is required. Every sidecar this build knows is
compiled in (`crates/holon-mcp-client/src/bundled_sidecars.rs`) — presence is a
compile-time fact, so a file on disk can neither introduce a provider nor switch
one on. `McpIntegrationsModule` in `crates/holon-app/src/mcp_integrations.rs`
loads the `IntegrationConfigStore` from the integrations directory (default
`{config_dir}/integrations/`, overridable via `HOLON_MCP_INTEGRATIONS_DIR`) and,
for each bundled provider whose state says `enabled = true`:

1. Parses an `IntegrationFileConfig` — transport, auth, entities, tools.
2. Expands `${VAR}` references, layered: environment variable first, then an
   app-settings value whose key matches case-insensitively with `.`/`_` as the
   same separator (`normalize_var_name`, so the `todoist.api_key` setting
   resolves `${TODOIST_API_KEY}`); empty string counts as unset. An unresolved
   `${VAR}` is a **disclosed skip** — the typed `UnresolvedVar` error
   (`integration_config.rs`) is caught, a warning is logged, and an inert
   `EmptyOperationProvider` is registered for that integration ("not configured
   yet, e.g. missing API key"). Any *other* config error (malformed YAML,
   structurally invalid config) fails loud with a panic. Connection failures
   are also warn-and-skip with the same inert provider.
3. Calls `build_mcp_integration()` which connects to the MCP server, builds a
   `QueryableCache<DynamicEntity>` Turso table for each entity that declares a
   `schema`, registers an `McpSyncEngine` with one strategy per entity's `sync`
   config, and wraps the whole thing as an `McpOperationProvider`.
4. Registers a `RegistryOperationProxy` into the DI `OperationProvider` set so
   the `OperationDispatcher` routes operations to the right integration.
5. Spawns a background initial sync and re-syncs individual entities when the
   MCP server sends resource-update notifications (MCP `subscribe` protocol).

For entities with a `vtable` config, two tables exist: the cache table itself
(e.g. `cc_session`, created by the cache factory when a schema is declared)
plus a Turso foreign table registered under the `_fdw`-suffixed name (e.g.
`cc_session_fdw` — see `mcp_integration.rs`), providing query-time MCP fetch
in addition to (or instead of) background sync. `fdw_backed_tables` holds the
plain cache-table names, and only for entities whose vtable has
`write_through: true`.

```
~/.config/holon/integrations/todoist.state.toml   (enabled = true)
  │  read by IntegrationConfigStore — the sole enablement authority
  ▼
load_integration_configs(dir, store)
  │  content: the bundled sidecar, or an installed *.yaml declaring
  │  this build's SIDECAR_SCHEMA_VERSION
  ▼
McpIntegrationsModule::from_dir()
  │  calls build_mcp_integration() per enabled provider
  ▼
McpIntegration { operation_provider, sync_engine, fdw_backed_tables, … }
  │  stored in McpIntegrationRegistry (DI singleton)
  ▼
RegistryOperationProxy  →  OperationDispatcher  (write path)
McpSyncEngine           →  initial sync_all() + notification re-sync
QueryableCache<DynamicEntity>  →  Turso cache tables (queryable via SQL/PRQL)
```

### Adding a New External System

1. Add a `*.yaml` sidecar to `assets/integrations/` and to `BUNDLED_SIDECARS`
   (see YAML Sidecar below for the full schema), then rebuild — sidecars are
   compiled in.
2. Switch it on: `scripts/holon-integration-enable.sh <provider>`, which writes
   `{config_dir}/integrations/<provider>.state.toml`.
3. Set any `${VAR}` secrets in the environment or in Holon's Settings UI
   (the key `todoist.api_key` maps to `${TODOIST_API_KEY}` automatically).
4. Restart the app.

An installed `*.yaml` that enables nothing — because the provider is off, or
because the build does not ship it — is disclosed at boot on the degraded bus
(WARN + toast) naming the state file to write. It is never silently ignored.

No Rust code is needed for an MCP-backed integration unless the MCP server
requires special connection handling (OAuth flows are already supported
generically via `AuthMode::OAuth`).

> **Future direction (F5 follow-up of the Cells plan)**: each external system will gain its own `EntityCellRegistry` impl alongside its `OperationProvider`. Consumers will then read entity field state through `services.cells().live_field::<T>(uri, field)` uniformly across local and external entities. The third-party API stays as the authority; cells project from the existing CDC stream. The cell infrastructure itself has landed — `EntityCellRegistry`/`live_field` in `holon-core` (`cell_registry.rs`, `traits.rs`) and `BlockCellRegistry` in `holon-loro` cover the `block` entity — so the remaining future work is the per-external-system registry impls. No changes to the integration story above.

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

| Component                  | File (holon-mcp-client unless noted)  | Purpose                                                                                                                                                                |
| -------------------------- | ------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `McpOperationProvider`     | `mcp_provider.rs`                     | Connects to MCP server, caches `OperationDescriptor`s from tool schemas, executes tools via `call_tool`. Holds `McpRunningService` to keep the connection alive.       |
| `McpSyncEngine`            | `mcp_sync_engine.rs`                  | Syncs MCP entities into Turso cache tables. Full-diff sync (insert/update/delete) or cursor-based incremental sync. Subscribes to MCP resource-update notifications.  |
| `McpForeignDataWrapper`    | `mcp_vtable.rs`                       | Turso FDW that translates SQL WHERE constraints into MCP tool parameters for query-time fetch (no background sync required).                                           |
| `IntegrationFileConfig`    | `integration_config.rs`               | Top-level YAML structure: `transport`, `auth`, `entities`, `tools`. `${VAR}` references expanded from env / app settings.                                             |
| `McpIntegrationsModule`    | `holon-app/mcp_integrations.rs`       | DI module: scans YAML dir, builds `McpIntegrationRegistry`, registers one `RegistryOperationProxy` per integration into the `OperationProvider` set.                  |
| `McpIntegrationRegistry`   | `holon-app/mcp_integrations.rs`       | DI singleton holding all live `McpIntegration` handles (keeps services alive) and the list of FDW-backed cache table names.                                            |
| `McpSidecar`               | `mcp_sidecar.rs`                      | Entity and tool annotations parsed from the YAML `entities`/`tools` maps: entity mapping, `affected_fields`, `triggered_by`, `precondition` (Rhai), `undo` config.   |
| `RhaiPrecondition`         | `mcp_sidecar.rs`                      | Parse-don't-validate wrapper: Rhai expressions are validated at YAML deserialization time. Invalid syntax fails immediately, not at operation execution.               |
| `mcp_schema_mapping`       | `mcp_schema_mapping.rs`               | Converts JSON Schema types to `TypeHint` (String, Bool, Number, OneOf, EntityId via overrides). Walks `inputSchema.properties` to build `Vec<OperationParam>`.        |
| `connect_mcp()`            | `mcp_provider.rs`                     | Establishes Streamable HTTP connection to an MCP server, returns `Peer<RoleClient>` + `McpRunningService`.                                                             |

#### YAML Sidecar

Each integration YAML file (`IntegrationFileConfig`) combines transport/auth
config with entity and tool declarations.  A representative example (the source
of truth for the full schema is `IntegrationFileConfig` in
`integration_config.rs` and `McpSidecar` in `mcp_sidecar.rs`):

```yaml
# Transport — one of http or child_process
transport:
  http:
    uri: https://ai.todoist.net/mcp      # ${VAR} expanded from env / settings
  # OR:
  # child_process:
  #   command: npx
  #   args: ["-y", "@my/server-mcp"]
  #   env: { MY_TOKEN: "${MY_TOKEN}" }

# Auth (HTTP only)
auth:
  static_token: "${TODOIST_API_KEY}"    # Bearer token; ${VAR} expanded at startup
  # OR: oauth: true                     # OAuth 2.1 — uses PendingOAuthFlows

# Optional prefix prepended to all entity/table names (e.g. "cc_" → cc_session)
# entity_prefix: "td_"

entities:
  todoist_tasks:
    short_name: task          # display name used in the UI
    # source_name: task       # server-side entity name when it differs from the YAML key
    id_column: id             # primary key column (default: "id")
    schema:                   # DDL for the Turso cache table
      - { name: id,       sql_type: TEXT, primary_key: true }
      - { name: content,  sql_type: TEXT }
      - { name: priority, sql_type: TEXT }
      - { name: parentId, sql_type: TEXT, indexed: true }
    # profile_variants: [...] # render variants, passed to TypeDefinition
    sync:                             # tool-based sync (list_tool present)
      list_tool: find-tasks           # MCP tool to call for bulk fetch
      extract_path: tasks             # JSON key in tool response containing array
      list_params: { filter: "all" }  # static params passed to list tool
      cursor:                         # optional cursor-based incremental sync
        request_param: cursor
        response_field: nextCursor
    # OR resource-based sync (list_resource present selects ResourceSync):
    # sync:
    #   list_resource: "tasks://{project_id}"
    #   uri_params: { project_id: "inbox" }
    # vtable:                         # alternative: foreign table (FDW) on the cache table
    #   # tool-based mode — SQL WHERE constraints pushed down as tool params:
    #   search_tool: find-tasks
    #   extract_path: tasks
    #   # OR resource-based mode — full fetch of a resource URI, NO pushdown:
    #   # list_resource: "tasks://{project_id}"
    #   # uri_params: { project_id: "inbox" }

tools:
  complete-tasks:
    entity: todoist_tasks
    affected_fields: [completed]
    triggered_by:
      - from: completed
        provides: [ids]
    precondition: "completed == false"  # Rhai expression, validated at parse time
    undo:
      reversible: false
  update-tasks:
    entity: todoist_tasks
    affected_fields: [content, description, priority, dueString, labels]
    # param_overrides: { ... }        # per-parameter TypeHint / display overrides
    undo:
      tool: update-tasks              # mirror undo: re-call same tool with old values
      capture: [content, description, priority, dueString, labels]
  add-tasks:
    entity: todoist_tasks
    display_name: Create Task
    undo:
      reversible: false
```

The top-level file is `IntegrationFileConfig`; the `entities` and `tools` maps
are deserialized into `McpSidecar` (in `holon-mcp-client`).  Tools without sidecar
entries still appear as operations, but with no gesture bindings (affected_fields,
triggered_by, preconditions).

#### Tool Name Normalization

MCP tools use kebab-case (`complete-tasks`), Holon operations use snake_case (`complete_tasks`). `McpOperationProvider` maintains a `tool_name_map` to translate between the two.

#### DI Registration

`McpIntegrationsModule` in `crates/holon-app/src/mcp_integrations.rs` performs
all DI wiring automatically from the YAML directory.  No per-integration Rust
code exists — the old `holon-todoist` crate has been deleted.

Key pieces registered by `McpIntegrationsModule::configure()`:

- **`McpIntegrationRegistry`** (async DI singleton): resolved concurrently with
  other DI services at startup, but builds the `McpIntegration` objects
  themselves sequentially (one awaited `build_mcp_integration()` per config —
  startup cost is additive per integration); resolves Turso `DbHandle` and
  `SyncTokenStore` from DI, runs initial `sync_all()` in a background
  `tokio::spawn`.
- **`RegistryOperationProxy`** (one per YAML file, added to the
  `dyn OperationProvider` set): delegates `operations()` and
  `execute_operation()` to the matching `McpOperationProvider` inside the registry.
- **`PendingOAuthFlows`** (root singleton): parked state for OAuth integrations
  awaiting user consent; the frontend calls `complete_oauth(provider_name, code, state)`
  after the browser callback.

After the registry is built, `wiring.rs` additionally:
- Calls `engine.register_fdw_table()` for every cache table that has FDW backing
  (from `McpIntegrationRegistry::fdw_backed_tables()`).
- Installs `sync_engine.clone()` as the `MatviewHook` so FDW cache tables
  subscribe to resource-update notifications at first access.

#### Reuse Across Integrations

`holon-mcp-client` is integration-agnostic.  The same infrastructure handles
Todoist, Claude History, and any future MCP server — just add a YAML file.  See
[Adding a New External System](#adding-a-new-external-system) above.

### Linking to Integration Entities

Blocks link to integration entities through the ordinary `[[…]]` link surface.
The link target is the entity's URI — the integration's entity name (with its
prefix) as the scheme, the foreign id as the path:

```org
See [[cc-session:0f3a1c88-…][refactor the matview lease]] for the trail.
```

**Classification is three-state.** A link target whose text has URI scheme
shape (RFC 3986: a letter followed by letters/digits/`+`/`-`/`.`, then a
colon with no space after it) is always an entity link, never a page name:

1. Scheme registered (a declared integration entity, or a built-in such as
   `block:`/`tag:`) → a resolved entity link.
2. Scheme not registered → an *unknown-scheme* link, rendered muted with the
   raw URI. It is never turned into a page.
3. Anything else — no colon, or a colon followed by a space (`[[Ketosis: How
   to lose weight]]`) — is a page link.

The scheme shape is reserved: page names may not look like URI schemes
(page creation rejects them and suggests `/` for hierarchy, e.g.
`Areas/Work`), and registering an integration fails loudly if an existing
page name collides with its scheme. Two reasons: this mirrors org-mode's own
typed-link semantics, so files stay portable to Emacs; and it makes
classification stable under configuration change — enabling or disabling an
integration only moves its links between *resolved* and *unknown-scheme*,
never across the page/entity boundary, and never rewrites bytes on disk.

**Resolution is total; presence is a display concern.** An entity link
resolves at parse time to its URI — it is never dangling and never waits for
a fetch. Whether the entity's data is *present* is disclosed at render:

| State | Condition | Rendering |
|---|---|---|
| Present | cache row exists for the URI | link label, or the profile's title column |
| Pending | scheme registered, row not yet fetched | the URI, muted, marked "not yet fetched" |

A pending link never shows a fabricated title. Rendering a link does not
trigger a fetch — priming stays with the entity page, so scrolling past a
link cannot fan out MCP calls.

**Backlinks and the entity page need no special machinery.** The backlinks
view keys on the resolved target id, whatever its scheme, so focusing an
entity lists every block that links to it. The entity's page is its single
cache row rendered through the entity's profile, followed by that backlinks
section — a parameterized query, not a page template.

**Entities stay out of block storage.** A foreign entity never gets a row in
the block tables: block rows imply an org-file home, CRDT replication, and
user-editable lifecycle — all owned by the user, while the entity is owned by
its provider. The dividing rule is *state lives in the store whose owner
authors it* (see Principles, "State Ownership"): the entity's own data lives
in its integration cache table; everything the user says *about* the entity —
notes, tags, extra properties — lives in ordinary blocks that link to it, and
the backlinks section aggregates them on the entity's page. A user who wants
an entity in their outline creates a block referencing it (an `entity::
<uri>` property plus an embedded view); every block operation then applies to
that block — moving, tagging, or deleting the *reference*, which is the only
coherent meaning, since the entity itself is not the user's to move or
delete.

## Frontend Architecture

Holon's primary frontend is **GPUI** — a native Rust desktop application. The Dioxus-web frontend (see below) is a **prototype**: the core works, but it is not actively tested. See [Engine.md §Supported Frontends](Engine.md) for the full status table.

### Dioxus-Web Frontend (Prototype)

Inversion of Control: the frontend asks for what to render, the backend
resolves everything. `holon-frontend` and `BackendEngine` run inside a
dedicated `wasm32-wasip1-threads` Web Worker (the `holon-worker` crate).
`frontends/dioxus-web` still depends on `holon-frontend`, `holon-api`, and
`holon-macros` at compile time for the shared render types — which is why it
needs its own rot guard (`just check-dioxus-web-wasm`) — but at RUNTIME the
process boundary carries nothing except the JSON wire format over
`postMessage`; there is no FFI and no shared engine handle. The backend
surface the worker drives (all signatures verified against the code):

```rust
// crates/holon/src/api/block_domain.rs — resolve a block into a render
// expression plus a CDC stream (first batch = initial query results):
impl BlockDomain {
    pub async fn render_entity(
        &self,
        block_id: &EntityUri,
        preferred_variant: &Option<String>,
    ) -> Result<(RenderExpr, RowChangeStream)>;
}

// crates/holon/src/api/ui_watcher.rs — watch a block's UI; re-renders via
// render_entity() on structural change, streams UiEvents + data deltas:
pub async fn watch_ui(engine: Arc<BackendEngine>, block_id: EntityUri)
    -> Result<WatchHandle>;  // WatchHandle: crates/holon-api/src/streaming.rs

// crates/holon-frontend/src/lib.rs — frontend dispatches user actions:
impl FrontendSession {
    pub async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: HashMap<String, Value>,
    ) -> Result<holon_api::OpOutcome>;
}
```

The frontend never sends queries — it only sends block IDs and receives render
instructions. Clicks become writes through one funnel: display builders build
an `OperationIntent`, `intent_to_wire()`
(`frontends/dioxus-web/src/editor.rs`) serializes it, and the worker's
`dispatch_intent_chain` runs it through `FrontendSession::execute_operation`.

### Reactive Updates

The frontend subscribes to `ViewModel` snapshots produced by
`ReactiveEngine::watch` (`crates/holon-frontend/src/reactive.rs`) inside the
worker, and bridges the `WatchEnvelope` messages into a Dioxus signal
(`frontends/dioxus-web/src/main.rs` `App()`). `WorkerBridge::on_snapshot`
deserializes each envelope into a `Signal<Option<ViewModel>>`, and `BootState`
flips to `Ready` only on the first envelope that actually carries a
projection — a subscription that never delivers must not read as green.

No explicit refresh calls — UI state derives from the change stream, and
Dioxus's fine-grained reactivity handles re-rendering.

### MCP Apps Rendering in Dioxus

> **Status: target architecture** — same caveat as the MCP Apps banner above; no `AppBridge` or `McpAppView` exists in the code yet.

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

The project uses **fluxdi** for service composition. The composition root is `crates/holon-app/src/wiring.rs` (`FrontendInjectorExt::add_frontend`), which assembles the full session with conditional Loro and OrgMode modules. See [Principles.md §Dependency Injection](Principles.md#dependency-injection) for details.
