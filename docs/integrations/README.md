# Connectors: external systems as Layer-1 replicas

This directory holds **integration sidecars** — declarative YAML files, one per
external system, that turn a third-party API into a first-class part of the
Holon block graph. A sidecar is the *only* surface for wiring an external system
in: there is no per-integration Rust (`holon-todoist`, a bespoke REST client, a
hand-written sync provider). Drop a `*.yaml` here (in production,
`~/.config/holon/integrations/`) and the generic connector engine
(`holon-mcp-client`) instantiates the replica.

This is the concrete realization of **Model.md Layer 1** ("the outside world as
replicas") and the `mcp-yaml-sidecars` directive: *MCP clients = declarative
YAML sidecars only.*

> Scope note: connectors today are **read-into-the-graph** replicas plus
> **explicitly-declared write tools** for MCP transports. General bidirectional
> sync with **leases / lease-governed external effects** (ADR 0024 Phase 4
> taxonomy) is an **open question** and out of scope — see
> [Open questions](#open-questions).

---

## 1. The mental model

An external system becomes a **replica**: a set of **entities** (resource kinds)
that project into **block-shaped rows** in Turso cache tables, kept fresh by a
**sync policy**, and mutated (where supported) by mapping intents onto the
provider's **tools**. Three mappings define a connector:

| Mapping | Sidecar section | Produces |
|---|---|---|
| **resource → block shape** | `entities.<name>.schema` | a typed cache table + a Holon entity type (a block shape) |
| **op → tool call** | `tools.<name>` | a user-invocable action mapped to a provider tool |
| **sync policy** | `entities.<name>.sync` (+ `views`) | how/when rows are fetched and derived |

The engine is **transport-plural**: the same fetch/extract/map/cache pipeline
runs behind one seam (`McpCallSurface`: `call_tool` + `read_resource`),
regardless of *how* the external system is reached. See
[§4 Transports](#4-transports).

---

## 2. Walkthrough: the Todoist connector

`todoist.yaml` is the fully-converted target — Todoist with no bespoke code.

### resource → block shape

```yaml
entities:
  todoist_tasks:
    short_name: task
    id_column: id
    schema:
      - { name: id,        sql_type: TEXT, primary_key: true }
      - { name: content,   sql_type: TEXT }
      - { name: priority,  sql_type: TEXT }
      - { name: projectId, sql_type: TEXT, indexed: true }
      - { name: parentId,  sql_type: TEXT, indexed: true }
      - { name: labels,    sql_type: TEXT, jsonb: true }
      # ...
```

Each entity becomes a cache table (`<entity_prefix><name>`) and a registered
Holon entity type. The `schema` columns are the block's fields; `id_column` is
the primary key, prefixed at the boundary with the entity's URI scheme so a
Todoist task id round-trips as a real Holon entity id. `parentId`/`projectId`
carry the foreign keys that let PRQL reconstruct the task hierarchy (see
`assets/queries/todoist_hierarchy.prql`).

### op → tool call

```yaml
tools:
  complete-tasks:
    entity: todoist_tasks
    affected_fields: [completed]
    triggered_by:
      - { from: completed, provides: [ids] }
    precondition: "completed == false"
    undo: { reversible: false }
  update-tasks:
    entity: todoist_tasks
    affected_fields: [content, description, priority, dueString, labels]
    undo: { tool: update-tasks, capture: [content, description, priority, dueString, labels] }
```

Each `tools.<name>` binds a provider tool to an entity and declares how a Holon
intent maps to it: which fields it affects, what triggers it, its precondition
(dual-evaluated Rhai guard), and its undo policy (irreversible, or an inverse
tool + the fields to capture for the inverse call). Tools with no write
semantics (`find-tasks`, `find-projects`) are declared so the schema/params can
be discovered but carry no mutation.

### sync policy

```yaml
    sync:
      list_tool: find-tasks
      extract_path: tasks
      list_params: { filter: "all" }
      cursor: { request_param: cursor, response_field: nextCursor }
```

`sync` says how to pull the replica: call `find-tasks`, pull the `tasks` array
out of the response, page via the `cursor` fields. A full sync (no cursor)
**diffs** against the cache — inserting new rows, deleting vanished ones — so a
server-side format change surfaces loudly as an error rather than as silent data
loss. Derived rollups are declared under a top-level `views:` block (each becomes
a Turso materialized view; see `claude-history.yaml` for the arg-max idiom).

### auth (secrets stay out of YAML)

```yaml
transport:
  http: { uri: https://ai.todoist.net/mcp }   # NOTE: MCP-over-HTTP, see §4
auth:
  static_token: "${TODOIST_API_KEY}"          # ${VAR} expanded from env at startup
```

`${VAR}` references are resolved from the environment (or a layered app-settings
resolver) at connect time and **fail loud if unset** — the secret never lives in
the file.

---

## 3. What the engine does with a sidecar

At startup `load_integration_configs(dir)` reads every `*.yaml`, and for each:

1. `IntegrationFileConfig::into_mcp_config` parses + type-checks it (serde
   `deny_unknown_fields` — a typo'd key fails loud), expanding `${VAR}`.
2. `build_mcp_integration` connects the chosen transport, creates the cache
   tables + entity types from `schema`, registers `tools` as operations, runs
   the initial sync, and subscribes for live updates.
3. `SyncStrategy::fetch_records` (over the `McpCallSurface` seam) fetches records
   for each entity; the engine maps each JSON record → a `DynamicEntity` (block)
   and writes it to the cache.

---

## 4. Transports

A sidecar picks exactly one transport under `transport:`. All plug into the same
connector engine behind the `McpCallSurface` seam.

| `transport:` key | Reaches | Notes |
|---|---|---|
| `child_process` | an MCP server over stdio | e.g. `claude-history.yaml` |
| `http` | an MCP server over **Streamable HTTP** | e.g. `todoist.yaml`. **`http` = MCP-over-HTTP**, historical name |
| `rest` | a plain HTTP/JSON API **directly** | UTCP-manual style; no MCP server. **read-only** |

> Naming caveat: `http` here means *MCP transported over HTTP*, not a generic
> REST call. The direct-API transport is `rest`. (UTCP can also describe MCP, so
> transports stay plural — `mcp | rest`, with `graphql` a possible future arm.)

### The `rest` transport (direct HTTP API)

Use `rest` when the external system has **no MCP server** — just a JSON API. The
sidecar carries a small UTCP-style manual: a `base_url`, an optional auth header
(referencing an env var — **never inline a secret**), and named `calls`, each a
GET endpoint. A `RestCallSurface` serves those calls behind the `McpCallSurface`
seam, so `sync.list_tool` names a `call` and the *entire rest of the pipeline is
unchanged*.

```yaml
transport:
  rest:
    base_url: https://api.example.com          # may be ${VAR}
    auth:                                       # optional
      header: Authorization
      value: "Bearer ${EXAMPLE_TOKEN}"          # env ref only, never a literal secret
    calls:
      list-things:
        method: GET                             # only GET today (read-only)
        path: /things/{ownerId}                 # {arg} filled from tool-call args
        query: { limit: "50" }                  # literals or {arg} placeholders
        result_key: things                      # wrap a bare-array body as {things: [...]}

entities:
  ex_things:
    id_column: id
    schema: [ { name: id, sql_type: TEXT, primary_key: true }, ... ]
    sync: { list_tool: list-things, extract_path: things }
```

- **`path` / `query` `{arg}` placeholders** are filled from the tool-call
  arguments at request time (distinct from `${VAR}`, which is a startup-time
  secret/config reference). A missing arg fails loud.
- **`result_key`** bridges REST's arbitrary top-level shapes to the tool-response
  contract: a bare array (`[ {...}, {...} ]`) is wrapped as
  `{ <result_key>: [ ... ] }` so `sync.extract_path` can select it. Omit it when
  the API already returns an object with the field.
- **Response → block shape**: the selected array's objects map field-by-field
  onto the entity `schema`, exactly as for the MCP transports.

Worked read-only example: **`jsonplaceholder.yaml`** — the public
`https://jsonplaceholder.typicode.com/posts` API (no auth), exercised
end-to-end against a local mock server in
`crates/holon-mcp-client/tests/rest_transport_mock.rs`.

---

## Open questions

- **Leases / read-write for `rest`.** The `rest` transport is read-only (GET
  only). Write-back — and the general **lease-governed external-effect** model
  (diffed intent against the replica's own base, ADR 0024 Phase 4 place-kind
  taxonomy) — is unresolved and out of scope. Non-GET methods fail loud today.
- **`rest` background runner.** The read path runs through the shared
  `SyncStrategy`/`McpCallSurface` seam, but the production background runner is
  built around MCP resource *subscriptions* (`Peer::subscribe`), which a plain
  HTTP API cannot serve. Wiring `rest` into a **poll-only** background runner is
  the remaining production step; `build_mcp_integration` fails loud for a `rest`
  sidecar until then.
- **Further transports.** `graphql` (and richer UTCP manuals: POST bodies,
  pagination cursors for `rest`) are natural extensions of the same seam.
