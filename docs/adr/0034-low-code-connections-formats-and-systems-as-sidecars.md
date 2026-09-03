# ADR 0034 — Low-code connections: formats and systems attach as sidecars, not as crates

**Status:** Accepted (directive 2026-09-02; D79.a, D80.a, D81.a, D82.c, D83.b,
D84.d ratified by Martin 2026-09-03)
**Date:** 2026-09-03
**Deciders:** Martin (directive on `crates/holon-kitchen`; decision-inbox
rulings D79–D84)
**Relates to:**
[ADR 0024](0024-unified-action-execution.md) — the Petri net is the one action
language; a connection produces rows, never its own execution path.
[ADR 0030](0030-birth-atomicity-authority-and-mirror-contract.md) — rows reach
storage only through the one authority.
`docs/Architecture/Model.md` — layer 1 (replicas) and invariant 4 (exactly one
writer per store).
`docs/Architecture/Integrations.md` — the external-system pattern and the MCP
sidecar precedent.

## Problem

Holon's promise is that a user attaches it to the formats and systems they
already use. The kitchen/shopping implementation
(`crates/holon-kitchen`, ~2 200 lines with `crates/holon-app/src/shopping_rest.rs`)
does the opposite: the recipe format has a bespoke Rust parser
(`cook.rs` 325 lines, `rows.rs` 132, `file_format.rs` 204) and one specific
third-party shopping app is a hard-coded client (`shopping.rs` 680,
`shopping_sync.rs` 357). Every next format and every next system would cost the
same again. The feature is not the point; being able to attach without writing
Rust is.

The two halves are not equally bespoke, and saying so is what shapes the answer:

| Concern | Where | Verdict |
|---|---|---|
| Wire contract (URL, verbs, query, body envelope, cadence, response version path) | `assets/integrations/shopping.yaml` (74 lines) | Already declarative |
| Generic REST transport | `crates/holon-mcp-client/src/rest_transport.rs` | Generic, keep |
| Response JSON → domain values | `crates/holon-kitchen/src/shopping.rs` | The real gap |
| Rows → command envelope | `crates/holon-kitchen/src/shopping_sync.rs` | The same gap, write direction |
| Reconciliation (tombstones, watermark, absence-as-deletion, idempotency) | `shopping.rs`, `shopping_sync.rs` | Generic for id-less lists; keep, rename |
| `.cook` parse → typed rows | `crates/holon-kitchen/src/cook.rs`, `rows.rs`, `file_format.rs` | Bespoke per format; replace with a plugin |

Both halves are missing the same primitive: a declarative mapping from JSON to
typed rows.

## Decision

### 1. First principles

A user attaches a format or a system by authoring **data**, not Rust. Rust is
written once per provider *kind* and amortised across every connection.
Optimisation targets, in order: zero Rust per connection; one generic host per
provider kind; minimum total Rust; one mapping language shared by both halves.

Three constraints bound every choice below. **Platform-complete** — macOS,
Android (GPUI native: no shell, no runtime compiler) and the `wasm32` web worker,
from one code path. **Fail loud** — an unsatisfiable sidecar errors at boot,
never degrades into a missing feature. **Secrets never in sidecars** — only
`${VAR}`, and writing `${VAR}` is what *marks* a value secret and strips it from
logs (`crates/holon-mcp-client/src/rest_transport.rs:50`,
`crates/holon-frontend/src/integration_vars.rs`).

### 2. The neutral contract: JSON Lines of typed rows

Every provider — a format plugin, a remote system, a future CLI tool — emits the
existing `TypedRowSet` (`crates/holon-core/src/file_format.rs:52`) on the wire as
JSON Lines, so the sink (`DispatchingTypedRowSink`,
`crates/holon/src/core/typed_row_sink.rs:34`) is unchanged.

```
{"holon_rows":1,"scopes":[{"type":"recipe","owner_column":"source_path","owner_value":"Rezepte/Pfannkuchen.cook"},
                          {"type":"ingredient_use","owner_column":"recipe_id","owner_value":"recipe:Rezepte/Pfannkuchen.cook"}]}
{"type":"recipe","row":{"id":"Rezepte/Pfannkuchen.cook","title":"Pfannkuchen","servings":"4|6","course":null}}
{"type":"ingredient_use","row":{"id":"…::iu::mehl-0","recipe_id":"recipe:…","raw_name":"Mehl","quantity":250.0,"unit":"g","step_index":0}}
```

Four rules, each a loud error when broken:

1. **Line 1 declares every scope.** A scope with zero following rows is legal and
   load-bearing — it is how the last row of a set gets swept. Inferring scopes
   from the rows present would make that unrepresentable.
2. **Replace-scope semantics**, as the adapter contract already has them.
3. **Ids derive from content, never position**; the host re-checks with
   `checked_local_id` promoted out of the kitchen crate
   (`crates/holon-kitchen/src/rows.rs:115`).
4. **An undeclared type or owner column is refused**, as `typed_row_sink.rs`
   already does.

JSON Lines first. CSV and Arrow only on a measured need.

### 3. One provider host: wasm on `wasmi`

**`wasmi` is the plugin runtime.** It is the only runtime that builds for all
three targets from one code path (pure Rust, `no_std`, documented
`wasm32-unknown-unknown` build) and its wasmtime-like API keeps a later swap
contained. Rejected: **extism**, which embeds wasmtime — whose runtime feature
does not compile to wasm, so the browser needs a separate JS SDK, i.e. two hosts
and two ABIs, and Android is wasmtime Tier 3 with no CI. **`wasm_runtime_layer`**
is a clean native/browser split but single-maintainer; it is the escape hatch if
interpreter speed bites.

The ABI is **core wasm, not the component model** — no runtime offers components
on all three targets. Four functions (`alloc`, `dealloc`, `parse`, `last_error`)
with JSON Lines through linear memory.

**Guests are pure functions.** Bytes plus context in, rows out: no filesystem, no
clock, no network. That capability model also disposes of the WASI question —
wasmi's WASI-p1-only limit never binds.

**tree-sitter is a plugin TEMPLATE, never a native host.** Runtime grammar
loading needs `wasmtime-c-api` (cannot target wasm32) and shells out to a C
compiler (dead on Android), so a native tree-sitter host can never be
platform-complete. Instead one generic guest per grammar, built from a template
(a pure-Rust tree-sitter runtime + the grammar + jaq), with the `.scm` query and
the jaq filter passed as *data* at call time. Adding a format is dropping a
grammar `.wasm` beside a sidecar. Stated cost: a *new* grammar needs a build
step, so grammars are low-code, not no-code.

A CLI host is desktop-only and refused loudly on Android and web, so a feature is
never silently missing. It gets built only when a real connection needs it.

### 4. One mapping language: jaq

`jaq` is the single mapping language, at four call sites: AST-JSON → rows,
response → rows, rows → commands, and vocabulary parsing. `jq`'s
`.. | select(…) | {…}` with stream-of-results semantics maps 1:1 onto "walk a
document, emit N rows", and a filter compiles once and runs over many values.
`jaq-core` is `#![no_std]` with pure-Rust dependencies and already runs in a
browser worker.

Rejected: `jsonata-rs` (incomplete, long unreleased), `jmespath` (no arithmetic,
no recursive descent), `serde_json_path` (selector only), and a language of our
own.

Rejected too, and this is the load-bearing rejection: a **field-path mapper**.
The evidence is in the code being replaced — the shopping vocabulary parser is
156 lines of Rust with zero constants, and a rename is delete-plus-add. Neither
is a field path. A field-path mapper covers one peer's easy 80% and then needs a
Rust escape hatch for every peer after it, which is the failure this decision
exists to end. Stated cost: users must learn jq, and its errors are terse.

### 5. Sidecar shape: a verbatim UTCP manual plus a `holon:` section

A connection sidecar has two top-level sections:

```yaml
utcp:                       # a VERBATIM UTCP 1.x manual; exportable unchanged
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  tools:                    # standard http call templates, ${VAR} secrets
    - name: commit
      tool_call_template: {call_template_type: http, url: "${LIST_URL}/commit", http_method: POST}
holon:                      # what the standard lacks
  poll_interval: 60s
  auth: {...}               # transport-wide, as before
  tools:                    # keyed by the manual's tool name
    commit:
      query: {version: "{version}"}
      body: {oldVersion: "{version}", device: {id: "{deviceId}"}, commands: "{commands}"}
      response: "<jaq: one response → a holon-rows stream>"
      request: "<jaq: a {scopes, rows} stream → this call's arguments>"
```

Per-tool entries sit under `holon.tools` rather than directly under `holon`, so
a peer that named a tool `auth` or `poll_interval` is still representable. The
worked example is `assets/integrations/shopping.yaml`.

The `utcp` section round-trips to and from any standard client, because it
contains no unknown keys — byte-for-byte, pinned by
`crates/holon-mcp-client/tests/utcp_manual_roundtrip.rs`. A user imports a published UTCP or OpenAPI manual and
authors only the `holon` section. Holon parses the manual with **its own serde
types** for spec 1.x — three top-level fields and one call-template struct —
and takes **no dependency on `rs-utcp`**. A tool named in `holon` but absent from
`utcp` fails loud at load.

This shape is what the measured shortfalls force. Verified against Python `utcp`
1.1.3 / `utcp-http` 1.1.11 with live local servers:

| Claim | Verdict |
|---|---|
| No request **body template**; `body_field` names one argument that becomes the whole body | Confirmed — with `body_field="commands"` the wire body was exactly `[{"op":"add"}]`; no envelope, no literal fields |
| No **response mapping in the manual** | Confirmed for the manual. A `ToolPostProcessor` interface exists but lives in *client config*, so it cannot travel with the integration; the two built-ins only prune keys or truncate strings |
| Query parameters are undeclared leftovers | Confirmed — `version` and `_nocache` reached the query string purely by elimination; the template names them nowhere |
| No polling **cadence** | Confirmed — no scheduling field anywhere in the schema, and no scheduling concept in the spec |
| A file-sourced manual silently registers ZERO tools while reporting success | Confirmed and reproduced; a fail-quiet footgun with a one-line workaround |
| `${VAR}` is substitution only, and a 401 leaks the resolved secret | Confirmed and worse: the secret appeared both in the exception text and in an ERROR log line |
| `rs-utcp` 0.3.2 tracks spec 0.3.0 while the reference client is 1.1.3 | Confirmed; the two disagree on the manual's top-level schema |

The decisive fact: **the reference client rejects unknown keys deliberately**, and
an unknown `call_template_type` fails validation of the WHOLE manual — a manual
holding one valid tool plus one extended tool registered zero tools. So a forked
manual is invalid UTCP for every standard client until upstream changes that
rule, whereas the `utcp:` section here is valid UTCP from day one. The two
designs converge the moment upstream merges, at which point the `holon` section
folds into the manual.

**Holon's own reader applies the ignore-unknown-keys rule NOW, ahead of
upstream.** A `utcp:` key this build does not model is ignored, PRESERVED so an
export gives it back, and disclosed with a warning naming the dotted key; a tool
whose `call_template_type` this build cannot drive is SKIPPED with a warning and
the rest of the manual still loads. That is deliberately the opposite of the
reference client's behaviour above, and it is what makes "import a published
manual unchanged" true rather than aspirational: a real 1.x manual carrying
`info`, `auth`, `query_params` or extra call-template fields loads as it stands.
Preservation is by content, not position — an unmodelled key is written back
after the keys this build names. Pinned by
`crates/holon-mcp-client/tests/utcp_manual_roundtrip.rs`.

The `holon:` section gets the OPPOSITE treatment and refuses unknown keys
loudly, with the accepted-field list. Those keys are ours: a typo there is a
mapping that would silently never run, where a tolerated stranger in `utcp:` is
merely a field we do not use.

Two upstream PRs — an ignore-unknown-keys rule and a manual-carried mapping — are
a side lane filed from a fork of the SPEC repository. **PR-1 is the upstream
mirror of the rule Holon's reader already applies**, not a prerequisite for it.
That fork is a staging area for contributions, never a runtime dependency. `rs-utcp` is not forked: it is a
major version behind, and Holon's own transport already does the envelope, query,
cadence and secret redaction it lacks.

### 6. Where sidecars live, and what is admitted

Sidecars live in a **device-local container with an explicit replicate flag** —
a connection is a property of the device that holds the credentials, and sharing
one is a deliberate act.

Admission follows the existing precedent: bundled copies compiled in
(`crates/holon-mcp-client/src/bundled_sidecars.rs`) plus user overrides from
`{config_dir}/connections/*.yaml`, behind the existing `schema_version`
generation gate. One typed load at boot with `deny_unknown_fields`, refusing
loudly on: an extension two adapters claim; an undeclared type or owner column; a
`${VAR}` with no preference definition
(`crates/holon-frontend/src/preferences.rs`); a non-HTTPS non-localhost URL; an
absent declared export; a `cli` provider where no shell exists.

A user-dropped `.wasm` guest is **allowed by default**. The guests are pure
functions with no ambient authority (§3), so the trust decision is about what the
rows say, not about what the guest can reach.

Write-back is `WriteTier::ReadOnly` by default
(`crates/holon-core/src/file_format.rs:248`). A sidecar earns `ReadWrite` only by
declaring a reverse export, and the `writeback_drops` grounding applies unchanged.

### 7. Where this sits in the model

Layer 1 already names external APIs and org files as replicas; a connection **is**
a replica, with the plugin or the manual producing `diff(base, current)` as rows
and the reconciler holding the base. Layers 2–3 are untouched: rows reach Turso
only through the one shared `OperationDispatcher` via `DispatchingTypedRowSink`,
so invariant 4 survives and no connector becomes a second writer.

## Consequences

- A new format costs a `.wasm` guest and a sidecar; a new system costs a manual
  and a jaq filter. Neither costs Rust.
- The reconciler is generic only for **id-less** lists, where identity is
  `(name, cat)` because the peer issues no id. Systems with server ids (Todoist,
  JIRA) need keyed rows, and a content-key reconciler would read a server-side
  rename as delete-plus-add and lose local state. The sidecar therefore declares
  the key derivation as a jaq expression — `.id` for a server id, `[.name,.cat]`
  for a content key — and one reconciler takes it as a parameter.
- Users must learn jq to author a mapping. That is the price of not needing an
  escape hatch per peer.
- A differential test between a plugin and the Rust parser it replaces must NAME
  every divergence and triage it as a bug-funnel entry (old wrong / new wrong /
  both wrong). "New ≡ old" as a pass criterion would silently bless upstream
  parse differences — the German timer units the current parser refuses are
  already a known entry. A divergence quietly absorbed is a gate failure.
- **`crates/holon-kitchen`'s bespoke parser and shopping client are DELETED**
  once the plugin path and the mapping layer land: `cook.rs`, `rows.rs`,
  `file_format.rs` and the `cooklang` dependency go with the format plugin;
  `shopping.rs`, `shopping_sync.rs` and `crates/holon-app/src/shopping_rest.rs`
  go with the generic mapping layer and the renamed `RemoteListReconciler`. No
  old path stays. That move also closes a platform hole — the write leg lives in
  `holon-app`, which is in neither wasm graph, so it is silently desktop-only
  today.
- Known kill criteria, each with a measurement rather than an opinion: wasmi too
  slow (a full recipe-directory scan against the current parser, with the 200 ms
  p95 interaction→projection SLO as the line); jaq cost, **stated per
  interaction and delta-bounded** — a full peer response is a poll or sync
  event, not an interaction, and the mapping runs off the interaction thread, so
  the line is what one interaction's DELTA costs, not what one cold full-snapshot
  costs. Filter-compile and per-response time are still measured separately: a
  filter recompiled per response is the defect, not jaq. MEASURED 2026-09-03
  (`crates/holon-kitchen/tests/shopping_mapping_cost.rs`): compile 1 ms once per
  connection; a 10 000-item full snapshot maps in 1266 ms, linear at ~0.13 ms
  per item, so a few hundred items cost single-digit milliseconds. **Ruled
  D92.a — accepted**; revisit via delta sync when a real peer exceeds ~1 500
  items. Also unchanged:
  `.wasm` guests bloating the APK and the worker bundle (a stated size budget
  before any guest ships).

## Out of scope

The component model; vault-level sidecars; plugin signing or a marketplace; CSV
and Arrow; replacing the sidecar's wire section with UTCP at *runtime*; iOS.
