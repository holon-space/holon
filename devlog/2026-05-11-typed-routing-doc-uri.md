# Phase 3.7 follow-up — typed `Event.routing_doc_uri`

Date: 2026-05-11

## TL;DR

Roadmap item 5 from `devlog/2026-05-11-095845-phase3.7-typed-position-event-field.md`
("Same typed-event treatment for `_routing_doc_uri`") landed. The OrgMode
event handler used to read `_routing_doc_uri` as a string from
`event.payload`; it now reads `event.routing_doc_uri.as_deref()` — a
typed `Option<String>` field on `Event`. `SqlOperationProvider` lifts the
value off the params bag (or its internal `find_document_uri` lookup)
onto the typed field at the operation boundary; `TursoEventBus`
round-trips it through the SQL events table via a transport key
analogous to `POSITION_AFTER_BLOCK_ID_PAYLOAD_KEY`.

The producer-facing `ROUTING_DOC_URI_KEY` param constant is unchanged —
`build_block_params` (in `holon-orgmode`) still adds the key to the
params HashMap. Same shape as the typed positional field: the param-side
key survives, the payload-side string is gone, the consumer reads the
typed view.

## What landed

### Typed Event field + transport key

`crates/holon/src/sync/event_bus.rs`:

- New `Event::routing_doc_uri: Option<String>` field, serde-default for
  backwards-compat with persisted events.
- New `Event::with_routing_doc_uri(self, Option<String>) -> Self` builder.
- New `pub const ROUTING_DOC_URI_PAYLOAD_KEY =
  "__transport_routing_doc_uri"` — transport-only payload key for the
  SQL round-trip.
- Doc on `ROUTING_DOC_URI_KEY` updated to clarify it is now the
  *param-side* key only (the producer-side hint), recognised by
  `SqlOperationProvider`'s `_routing_` prefix branch.

### Transport round-trip

`crates/holon/src/sync/turso_event_bus.rs`:

- `publish` / `publish_batch`: flush `event.routing_doc_uri` into payload
  under `ROUTING_DOC_URI_PAYLOAD_KEY` just before serialise. `publish`
  mutates the caller's Event (it already does so for the position
  transport key); `publish_batch` clones the payload locally.
- `parse_row_change_to_event`: remove the transport key from payload
  and lift it back onto the typed `routing_doc_uri` field so downstream
  consumers see the typed view only.

### Provider-boundary lift

`crates/holon/src/core/sql_operation_provider.rs`:

- `publish_event` now takes `routing_doc_uri: Option<String>` and
  attaches it via `.with_routing_doc_uri(...)`.
- `build_event_payload` skips `ROUTING_DOC_URI_KEY` from payload (same
  short-circuit pattern as `POSITION_AFTER_BLOCK_ID_PARAM`). Without
  this skip the `_routing_` prefix branch above would re-add it to
  payload and the consumer would still see two channels.
- `prepare_create` lifts the typed `routing_doc_uri` from params onto
  the Event (mirrors `position_after_block_id`).
- `prepare_delete`: drops the payload-side routing insertion; attaches
  the typed field directly on each cascade-delete Event.
- `execute_operation`'s `set_field` / `create` / `update` arms: stopped
  inserting `ROUTING_DOC_URI_KEY` into payload; thread the
  `find_document_uri(&id).await` result onto the typed Event field via
  `publish_event`.
- `execute_batch_with_origin`'s post-SQL Update-event construction
  threads the typed field on the `make_event` builder chain.

### Consumer

`crates/holon-orgmode/src/di.rs::extract_doc_ids_from_event`:

- Primary read switched from `event.payload.get(ROUTING_DOC_URI_KEY)`
  to `event.routing_doc_uri.as_deref()`.
- Fallback to `data.parent_id` retained for events lacking the typed
  hint (e.g. Loro outbound batched creates that don't go through
  `find_document_uri` at the operation boundary).

### Producer doc tidy

`crates/holon-orgmode/src/block_params.rs`:

- `build_block_params` doc-comment updated to describe the
  param→typed-field handoff explicitly.

## Why follow Phase 3.7's pattern verbatim

Two reasons to reuse the exact transport-key / typed-field shape:

1. The persistence problem is identical. The events SQL table has a
   fixed column set; new typed Event fields have to ride through
   payload JSON or get their own column. A transport key plus
   producer-side flush + consumer-side strip is the cheapest option
   that keeps the typed view "structural" everywhere except inside
   `TursoEventBus`.

2. Future cleanups (Stage 2-plus, item 7 from the Phase 3.7 roadmap —
   archlint forbidding `_routing_*` payload keys) become a regex over
   payload-side string lookups. Keeping both typed fields shaped
   identically lets one lint cover both.

## Verification

| Check | Status |
|---|---|
| `cargo check --workspace --tests` | GREEN |
| `cargo test -p holon-core --lib block_operations_tests` | 19/19 |
| `cargo test -p holon --lib sync::loro_sync_controller` | 16/16 |
| `cargo test -p holon --lib sync::block_cell_registry` | 5/5 |
| `cargo test -p holon --lib sync::turso_event_bus` | 3/3 |
| `cargo test -p holon --lib core::sql_operation` | 2/2 |

Pre-existing test failures (verified by running the same tests against
the parent commit `da512337`):

- `holon --lib api::backend_engine::tests::test_execute_operation` —
  "No provider registered for entity: test_item"; unrelated to routing
  or Phase 3.7.
- `holon --lib api::loro_backend_pbt` and `api::sync_pbt` — proptest
  fork/persistence harness panics ("child process crashed before first
  test started").
- `holon-orgmode --lib file_watcher::tests::test_file_watcher_respects_gitignore`
  — macOS FSEvents timing flake.
- `holon-orgmode --features di sync_controller_mutation_pbt::test_sync_block_change_to_file`
  + `test_sync_file_change_to_blocks` — same minimal failing input
  appears on parent commit.

The Full-mode `general_e2e_pbt` flake (Phase 3.7's "Open follow-up",
the phantom-Loro-exists race) is structurally unrelated to this
change. It remains the gate for the inbound-runtime gate flip.

## What did NOT change

- `Event::routing_doc_uri` is `Option<String>`, not `Option<EntityUri>`.
  Phase 3.7's devlog suggested `EntityUri`, but the consumer parses the
  string into `EntityUri` itself, and the producer (`build_block_params`)
  has the URI as a `String` anyway via `document_uri.to_string()`. The
  String shape matches `position_after_block_id` for consistency and
  avoids dragging `holon_api::EntityUri` into `event_bus` as a load-
  bearing type for serde.

- The producer-side `ROUTING_DOC_URI_KEY = "_routing_doc_uri"` param
  constant is retained. `SqlOperationProvider::partition_params`'s
  `_routing_` prefix branch still recognises it as operation-control
  metadata (skip from SQL columns). The intent is that *producers*
  continue using the params HashMap as their generic dispatch API; the
  provider lifts the value at the boundary.

- Loro outbound (`LoroSyncController::on_loro_changed` →
  `block_to_params`) does NOT add `ROUTING_DOC_URI_KEY` to its emitted
  ops. As before, those events arrive at the consumer with no typed
  routing hint, and the parent_id-from-data fallback takes over. This
  is unchanged behaviour; the fix would be a separate look-up on the
  Loro side and is out of scope here.

## Stage-2-plus roadmap status

Of the seven items in Phase 3.7's "Roadmap to gate flip + post-flip
cleanups":

| # | Item | Status |
|---|---|---|
| 1 | Resolve phantom-Loro-exists flake | **OPEN** — gates gate flip |
| 2 | Flip the gate | blocked on (1) |
| 3 | Gate integration test | blocked on (2) |
| 4 | Retire sort_key write path entirely | blocked on (2) |
| 5 | Typed `_routing_doc_uri` | **LANDED (this PR)** |
| 6 | Chord-op direct positioning via cell registry | blocked on (4) |
| 7 | archlint rule for new `_routing_*` payload keys | **LANDED (this PR)** |

Item 7 (archlint rule) is now actionable: with both typed fields
landed, all the *consumer-facing* payload reads of `_routing_*` keys
are gone. The rule would forbid any new `payload.get("_routing_…")`
or `payload.insert("_routing_…")` outside of `SqlOperationProvider`'s
partition / build_event_payload internals.

### Item 7 landed in the same session

Added `archlint/smells/words.toml::[routing_payload_key]`. The rule
fires on any full-key string literal `"_routing_<name>"`. The two
remaining prefix-filter sites in `SqlOperationProvider`
(`partition_params`, `build_event_payload`) use the bare prefix
`"_routing_"` and so don't match the full-key regex. The canonical
const definition at `crates/holon/src/sync/event_bus.rs` is excluded
via the rule's `exclude` field.

Positive test: a synthetic `"_routing_after_block_id"` literal trips
the rule; adding `// ALLOW(routing_payload_key): <reason>` silences it.
Negative test: `archlint.py --all` over the full repo reports zero
`routing_payload_key` violations.

## Files touched

- `crates/holon/src/sync/event_bus.rs` — typed field + builder +
  transport-key constant + doc updates.
- `crates/holon/src/sync/turso_event_bus.rs` — payload-transport
  round-trip on both `publish` paths; deserialiser lifts back to typed
  field.
- `crates/holon/src/core/sql_operation_provider.rs` — boundary lift in
  prepare_create, prepare_delete, publish_event, execute_operation
  arms, execute_batch_with_origin's update path; build_event_payload
  skips the routing key from payload.
- `crates/holon-orgmode/src/di.rs::extract_doc_ids_from_event` — reads
  typed field, fallback to parent_id-in-data unchanged.
- `crates/holon-orgmode/src/block_params.rs` — doc-comment update only.
- `archlint/smells/words.toml` — new `routing_payload_key` smell guarding
  the deletion above. Excludes `crates/holon/src/sync/event_bus.rs` (the
  canonical const definition site).
