---
id: 2026-09-12-an-integer-id-column-makes-every-connection-sync-fail-with-datatype-mismatch
date: 2026-09-12
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  A sidecar entity whose id column is declared INTEGER — the shape the bundled
  jsonplaceholder example teaches — fails every sync batch with "datatype
  mismatch" forever, because the sync engine rewrites the id into a prefixed
  string before the insert.
---

## Bug

Found by the `dogfood-explorer` gate for `user-connections` (main
`f134df9ece6c`), driving the real GPUI app against a loopback mock.

A connection `fixturebox` was introduced by dropping a file into the sandbox
integrations directory. Its two entities copy the bundled
`assets/integrations/jsonplaceholder.yaml` schema shape:
`{ name: id, sql_type: INTEGER, primary_key: true }`, fed by a JSON array whose
`id` values are JSON numbers.

The connection enables, connects, and authenticates — the mock's access log
records `auth_ok=True` on both tools — and the sync engine reports fetching the
rows:

```
[ToolSync] Got 2 records from 'list-items'
sync_entity: fetched records records=2 entity="fb_items"
[QueryableCache] Applying batch of 2 changes to table: fb_items
WARN ... poll resync failed error=Batch transaction failed: Database error:
     Failed to execute statement: datatype mismatch
```

`SELECT count(*) FROM fb_items` returns 0. The failure repeats on every poll,
indefinitely.

Isolated in the same tree with a second connection, `fixturetext`, identical
except `id` declared `TEXT` and fed string ids: it syncs, and
`SELECT count(*) FROM ft_items` returns 2. Evidence:
`scratchpad/dogfood-uc/logs/app2.log` (INTEGER, failing) and `logs/app3.log`
(both; 3 mismatch lines, all from the INTEGER entity).

## Root cause

`crates/holon-mcp-client/src/mcp_sync_engine.rs:40-41,74`. Before a fetched
record reaches storage, the id column is rewritten into a scheme-prefixed
identifier and stored as a string:

```rust
Value::String(raw) => Some(format!("{scheme}:{raw}")),
Value::Integer(n)  => Some(format!("{scheme}:{n}")),
...
Some(prefixed) => entity.set(key.as_str(), Value::String(prefixed)),
```

So a row whose `id` arrived as the JSON number `1` is written as the string
`"fb-items:1"` into a column the sidecar declared `INTEGER PRIMARY KEY NOT
NULL`, and Turso refuses the statement. The engine handles `Value::Integer`
explicitly at three sites, so integer ids were consciously anticipated on the
way IN; what was not reconciled is that the prefixing makes the stored type
unconditionally TEXT while the sidecar author still declares the source type.

The bundled `jsonplaceholder.yaml` — the file the rewritten
`docs/Architecture/Integrations.md` points a user at as the example to copy —
declares exactly this shape. So the documented template for authoring your own
connection is the broken one. This pass did not enable the bundled
jsonplaceholder itself, because that would call the public internet; the
evidence is the byte-identical shape against a loopback mock.

## Missing piece

ENVIRONMENT. `crates/holon-mcp-client/tests/rest_transport_mock.rs` drives the
REST transport against a local mock, and `entity_mirror.rs` has a unit test
(`integer id columns key the same way as strings`) that pins the MIRROR key
derivation for integer ids. Neither carries the value through to a real Turso
table whose column type came from the sidecar's declared schema, which is where
the two halves disagree. The mirror-key test in particular reads as coverage of
this exact concern and is not — it asserts on the key, never on the insert.

Secondary COVERAGE: no test declares an INTEGER-typed sidecar column at all
outside the bundled yaml files, so the generator could not have produced the
state either.

The keystone PBT cannot reproduce this: it has no connection-sync transition and
no sidecar-declared entity tables. Parity work needed: a rung that loads a
sidecar, serves its tools from a loopback mock, and asserts the declared rows
LAND in SQL — for each `sql_type` the sidecar schema accepts, id column
included.

## Remedy

FIXED. The reviewer's second direction was taken — parse, don't validate.

Both write legs store the identity column scheme-prefixed: the sync engine's
`prefixed_id` (`crates/holon-mcp-client/src/mcp_sync_engine.rs`) and the vtable
writeback. The value that reaches SQL is therefore a string whatever the source
JSON held, so the column's stored type is TEXT by construction and a declaration
of any other type is a claim the store cannot honour. Dropping the prefix for
integer columns was rejected: it would make the id's spelling depend on its
declared type and split the identity the entity-URI joins and the mirror key
both rest on.

- `MirrorSchema::parse` (`crates/holon-mcp-client/src/mcp_sidecar.rs`) now takes
  the entity's `id_column` and refuses a non-TEXT declaration at load, naming the
  entity, the column and the remedy type. Both producers pass it: the authored
  path (`parse_entity_schemas`) and the auto-discovered one
  (`absorb_discovered_entity`).
- `assets/integrations/jsonplaceholder.yaml` declares `id` as TEXT, with a
  comment saying why a numeric source id is still a TEXT column. The authoring
  guide (`docs/Architecture/Integrations.md`) says the same at the schema
  example. So the documented template no longer teaches the failing shape.

The ENVIRONMENT gap is closed by the parity rung the entry asked for:
`crates/holon-mcp-client/tests/sidecar_id_column_is_text.rs` builds a real Turso
cache table from the sidecar's own declaration and inserts the id the engine
actually stores, for EVERY id `sql_type` the loader accepts. Before the fix it
went red with the production error verbatim — `id type INTEGER: the loader
accepted this declaration, but the id the engine stores (\`fx-items:1\`) does not
go into the column it built: Database error: Failed to execute statement:
datatype mismatch` — which is the same string the live app logged every ten
seconds.

WHICH DISCLOSURE THE USER SEES. The rule first fired only when the sidecar was
built, which is at CONNECT — so a bad file surfaced as
`IntegrationConnectFailed`, a toast that reads "the peer is down" rather than
"your file is wrong". The check now also runs at LOAD for user-installed files
(`choose_content_for` in `integration_config.rs` runs `McpSidecar::from_yaml`,
the single source of these rules, and maps a failure to
`IgnoredReason::Unusable`). A refused file therefore reaches the same
load-time disclosure as every other file defect — inline secret, wrong
schema_version, foreign secret namespace — and the app boots without it. Bundled
sidecars are untouched by that path.

Both PRODUCERS of a mirror schema are pinned, not just the authored one: a
discovered resource template is untrusted input that becomes a real cache table,
and replacing the id-column argument at the auto-discovery call site used to
leave the whole suite green. `discovered_schema_id_column_tests` in
`mcp_integration.rs` now refuses a discovered INTEGER id and names the resource
template it came from.

Side effect worth noting: `jp_posts` was the only rowid-alias cache table in the
tree, so `cache_tables_are_not_rowid_alias_tables.rs` now records an empty list.
That hazard is now unreachable through an identity column.

## Attribution

PRE-EXISTING, not a `user-connections` regression. Verified by reading the tree
at `a5e161c0` (the commit before that lane): every line named above is already
there — `git show a5e161c0:<path>`. What the lane changed is reachability: it
made the files user-supplied, so a shape that had only ever been authored
in-tree became one a user can write.
