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

**Installed file = the switch, not the schema.** Every sidecar in this directory
is compiled into the binary (`crates/holon-mcp-client/src/bundled_sidecars.rs`)
and declares `schema_version`. A file in `~/.config/holon/integrations/` turns
its provider ON; for a provider the build ships, the file supplies CONTENT only
when its `schema_version` matches the running build's `SIDECAR_SCHEMA_VERSION`.
Otherwise the bundled copy runs and the app discloses which installed file was
ignored and why — a copy taken before a format requirement landed can no longer
silently outrank the sidecar the engine was built against. Bump
`SIDECAR_SCHEMA_VERSION` (and every file here) whenever a requirement lands that
an older sidecar does not satisfy.

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

#### Auth: `static header` | `oauth2`

The `rest` transport's `auth:` is one of two arms (secrets never inlined):

```yaml
transport:
  rest:
    auth:                                       # static-header arm (back-compat)
      header: Authorization
      value: "Bearer ${EXAMPLE_TOKEN}"          # ${VAR} from env / settings
```

```yaml
transport:
  rest:
    auth:
      oauth2:                                   # OAuth2 refresh-token grant
        token_url: https://oauth2.googleapis.com/token
        client_id_env: GCAL_CLIENT_ID           # or client_id_file
        client_secret_env: GCAL_CLIENT_SECRET   # or client_secret_file
        refresh_token_file: ~/.config/holon/gcal-refresh-token   # mode 0600
        scopes: [https://www.googleapis.com/auth/calendar.readonly]  # informational
```

The `oauth2` arm exchanges a long-lived **refresh token** for short-lived
**access tokens** at `token_url`, caches them in memory (refreshing at ~90% of
their lifetime and once more on a 401), and attaches
`Authorization: Bearer <token>`. Security invariants: access tokens **never**
touch disk (only the refresh token is a file, written by *your* bootstrap
helper, never by Holon); a group/world-readable credential file is **refused
loudly** at startup; no token or secret is ever logged (error messages redact
token-request query strings and never echo request/response bodies). A missing
env var / absent refresh-token file is a disclosed skip ("not configured yet");
a misconfigured one (bad perms, unreadable, empty) is a hard error. Nothing
Google-specific lives in the engine — `gcal.yaml` and `gmail.yaml` are its
consumers, sharing one provider-parameterized
`scripts/google-oauth-bootstrap.sh`.

#### Query enrichment: now-tokens, pagination, field projection

Three generic knobs cover common JSON-API needs (each optional, opt-in):

- **Now-tokens** in any `path`/`query` value: `{now}`, `{now-1d}`, `{now+14d}`,
  `{now+30m}` render to an RFC 3339 UTC timestamp at request time (a rolling
  window without a dynamic clock in YAML). Distinct from `${VAR}` (startup
  secret) and `{arg}` (per-call data); a malformed offset fails loud.
- **Pagination** on a `json` call follows a response continuation token across
  pages, concatenating each page's item array, bounded fail-loud by `max_pages`:

  ```yaml
  calls:
    list-events:
      method: GET
      path: /calendars/{calendar_id}/events
      query: { timeMin: "{now-1d}", timeMax: "{now+14d}", singleEvents: "true" }
      pagination:
        items_path: items          # array concatenated across pages
        next_token_path: nextPageToken
        token_param: pageToken     # sent as ?pageToken=<token> on the next call
        max_pages: 50              # exceeding it (token still present) is a loud error
  ```

- **Field projection** under an entity's `sync:` lifts nested JSON scalars into
  flat, comparable columns (and derives simple flags) — needed when an API
  nests values (e.g. Google's `start.dateTime`):

  ```yaml
  sync:
    list_tool: list-events
    extract_path: items
    project:
      start:   { path: ["start.dateTime", "start.date"] }  # first present wins
      all_day: { exists: "start.date" }                     # 1 if present, else 0
  ```

Worked read-only example: **`jsonplaceholder.yaml`** — the public
`https://jsonplaceholder.typicode.com/posts` API (no auth), exercised
end-to-end against a local mock server in
`crates/holon-mcp-client/tests/rest_transport_mock.rs`.

#### Freshness: `rest` polls (no subscriptions)

A plain HTTP API cannot push `resources/updated` notifications, so the `rest`
transport runs a **poll-only** background runner: every sync entity is refreshed
on a fixed cadence, and each poll **diffs** against the engine's in-memory
mirror, so an unchanged response applies nothing (no needless cache churn). The
cadence resolves per entity, most specific first:

1. the entity's own `sync.interval` (e.g. `sync: { interval: 60s }`),
2. the transport-wide `transport.rest.poll_interval`,
3. the built-in default (**300s**).

```yaml
transport:
  rest:
    base_url: https://api.example.com
    poll_interval: 5m          # transport-wide default cadence for rest sync entities
    calls:
      list-things: { method: GET, path: /things }

entities:
  ex_things:
    id_column: id
    schema: [ { name: id, sql_type: TEXT, primary_key: true }, ... ]
    sync:
      list_tool: list-things
      extract_path: things
      interval: 60s            # overrides poll_interval for THIS entity
```

Both accept an integer (seconds) or a humantime-style string (`"30s"`, `"5m"`,
`"1h"`). REST has no subscription freshness, so leaving both unset does **not**
mean "never refresh" — the 300s default bounds staleness rather than silently
letting the replica go stale.

#### Response formats: `json` | `atom` | `rss` (feeds)

A `rest` call decodes its response body per a `format:` codec (default `json`,
back-compatible). The transport axis (how you reach the source) stays orthogonal
to the body codec (how the response is shaped), so **syndication feeds need no
new transport** — an Atom/RSS feed is fetched exactly like any GET, only the
codec differs:

- `format: json` (default) — the JSON path described above.
- `format: atom` — decodes an Atom feed (RFC 4287); each `<entry>` →
  `{ id, title, updated, author, link, content }` (falls back to `<summary>`
  when there is no `<content>`).
- `format: rss` — decodes RSS 2.0; each `<item>` → the same record shape
  (`<guid>`→id falling back to `<link>`, `<pubDate>`→updated,
  `<author>`/`<dc:creator>`→author, `<description>`/`<content:encoded>`→content).

Because a feed is inherently a collection, the decoded entry array is always
wrapped under `result_key` (default `entries`), so `sync.extract_path` selects
it just like the JSON case. The codecs are parse-don't-validate at the boundary:
the root element is asserted to be a feed and malformed XML fails loud; an empty
feed (zero entries) is legitimate.

Feed example sidecar (a blog's Atom feed, no auth):

```yaml
transport:
  rest:
    base_url: https://blog.example.com
    calls:
      list-posts:
        method: GET
        path: /feed.atom
        format: atom          # or: rss
        result_key: entries

entities:
  blog_posts:
    short_name: post
    id_column: id
    schema:
      - { name: id,      sql_type: TEXT, primary_key: true }
      - { name: title,   sql_type: TEXT }
      - { name: updated, sql_type: TEXT }
      - { name: author,  sql_type: TEXT }
      - { name: link,    sql_type: TEXT }
      - { name: content, sql_type: TEXT }
    sync:
      list_tool: list-posts
      extract_path: entries

tools: {}
```

Both codecs are exercised end-to-end against a local mock server (Atom + RSS
fixtures, no network) in `crates/holon-mcp-client/tests/rest_transport_mock.rs`.
XML is decoded with `roxmltree` (a lightweight read-only DOM parser already in
the dependency tree).

---

## Open questions

- **Leases / read-write for `rest`.** The `rest` transport is read-only (GET
  only). Write-back — and the general **lease-governed external-effect** model
  (diffed intent against the replica's own base, ADR 0024 Phase 4 place-kind
  taxonomy) — is unresolved and out of scope. Non-GET methods fail loud today.
- **`rest` background runner.** ✅ Done — `build_mcp_integration` now wires a
  `rest` sidecar into a **poll-only** background runner (`finish_rest_integration`:
  no MCP peer, no subscriptions, one poll ticker per sync entity; see
  [Freshness](#freshness-rest-polls-no-subscriptions) above). It rejects the
  out-of-scope shapes loudly at connect: `vtable`/`write_through` (needs an MCP
  peer to back the FDW cursor) and `sync.list_resource` (REST serves GET *calls*,
  not MCP resources).
- **Further transports.** `graphql` (and richer UTCP manuals: POST bodies,
  pagination cursors for `rest`) are natural extensions of the same seam.
