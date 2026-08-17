---
id: 2026-08-05-unpinned-scan-uuid-keyed-vtable-entity
date: 2026-08-05
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  An UNPINNED scan of a uuid-keyed `write_through` vtable entity fails loud:
  `SELECT uuid FROM cc_agent_message_fdw` (and `cc_message_fdw`) errors
  `[McpCursor] id column 'id' not in schema columns ["uuid", "session_id",
  ...]`.
source_line: 952
---

## Bug

(mirror-as-storage Increment 2, driving the REAL `claude-code-history-mcp`
binary through `build_mcp_integration` for the first time) **An UNPINNED
scan of a uuid-keyed `write_through` vtable entity fails loud: `SELECT uuid
FROM cc_agent_message_fdw` (and `cc_message_fdw`) errors `[McpCursor] id
column 'id' not in schema columns ["uuid", "session_id", ...]`.**
`McpCursor::delete_stale_children` takes its id column from `id_scheme`,
which `finish_integration` builds from
`EntityConfig::id_column_or_default()` — whose `"id"` is, in that method's
own words, "an answer to a question the entity never asked".
`message`/`agent_message` declare `uuid` via `primary_key: true` and no
`id_column`, so the default names a column their schema does not have. Same
root class as the hard-coded-id identity panic closed at ffddfd8b, in the
one call site that still reads the default instead of the declaration.
Latent in prod today because EVERY shipped render PINS the parent key from
WHERE, which runs no enumeration and never reaches the stale-deletion path —
and load-bearing tomorrow, because mirror-as-storage makes the unpinned
fan-out the normal read. Second-order: the same mismatch means
`cc_message.uuid`/`cc_agent_message.uuid` are stored UNPREFIXED (the
scheme-prefix branch keys on the same absent column), diverging from the
sidecar's own stated "each entity's OWN key scheme-qualified" convention.

## Root cause

secondary ENVIRONMENT: unpinned scan of a uuid-keyed write_through vtable
entity fails loud — `SELECT uuid FROM cc_agent_message_fdw` (and
cc_message_fdw) errors `[McpCursor] id column 'id' not in schema columns
["uuid", ...]`, because delete_stale_children reads its id column from
id_scheme, built from id_column_or_default()'s "id" default rather than from
the entity's declared identity (`uuid` via primary_key). Same class as the
ffddfd8b identity panic, at the one call site still reading the default.
Latent today ONLY because every shipped render PINS the parent key, which
runs no enumeration and never reaches the deletion path; mirror-as-storage
makes the unpinned fan-out the normal read, so it blocks Increment 4/6 for
both uuid-keyed entities. No layer had ever scanned these tables without a
WHERE clause. NOT fixed here: deriving the column from identity_columns()
also changes whether uuid values are scheme-prefixed in the cache tables
that shipped live_query SQL and the session_status views read — a ruling,
not a side effect. Pinned as characterization
(claude_history_identity.rs::an_unpinned_scan_of_a_uuid_keyed_entity_fails_loud_today)
asserting the exact error, so the fix makes it fail and say so)

## Missing piece

No automated layer ever scanned these foreign tables UNPINNED — the mock e2e
suite drives pinned renders and the fdw unit tests build their own
single-column-`id` fixtures, so the uuid-keyed + fan-out + write_through
combination was unreachable. Missing piece = a scan of every shipped
`write_through` vtable entity with NO WHERE clause, which is exactly what
the retarget will do. Secondary ENVIRONMENT: it needs the real provider's
multi-parent data to fan out at all.

## Remedy

OPEN 2026-08-05 — diagnosis only, deliberately NOT fixed in this lane (the
obvious fix, deriving the id column from `identity_columns()`, changes
whether `uuid` values are scheme-prefixed in the cache tables, which the
shipped `live_query` SQL and the `session_status` sidecar views read — that
is a ruling, not a side effect). Pinned as characterization by
`crates/holon-mcp-mock/tests/claude_history_identity.rs::an_unpinned_scan_of_a_uuid_keyed_entity_fails_loud_today`,
which asserts the exact error string so the day it is fixed the test fails
and says to move `message`/`agent_message` into the live measurement. BLOCKS
mirror-as-storage Increment 4/6 for both uuid-keyed entities.
