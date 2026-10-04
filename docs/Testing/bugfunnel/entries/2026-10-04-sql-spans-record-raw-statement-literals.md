---
id: 2026-10-04-sql-spans-record-raw-statement-literals
date: 2026-10-04
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Spans, log lines and error messages across the storage stack recorded SQL
  statements and operation parameters verbatim, so note bodies, property values
  and search terms reached stdout, the HOLON_LOG file and the OTLP span
  exporter.
---

## Bug

A statement reaches the database with its values inline whenever the caller
built the text instead of binding parameters, which
`BackendEngine::inline_parameters` does for every watched query. A note's body,
a property value or a search term therefore sits in the statement text, and
every site that logged that text logged the value.

The production filter is `holon_gpui=info,holon=info,holon_tui=info`. EnvFilter
matches a directive target as a string prefix, so `holon=info` enables
`holon_turso::turso` and `holon::api::*`. The leaking sites were at INFO, WARN
and DEBUG: live by default, with no opt-in, on every destination the frontend
installs — stdout, `HOLON_LOG=file://…`, and the OTLP span exporter
(`crates/holon-frontend/src/logging.rs:348`), which sends them off the machine.

Found by a security audit of the lane `sec-sql-spans`, then by that lane's
VERIFIER, which refuted the audit's own first fix. Three families:

1. **Span fields.** `DbHandle::query` / `execute` / `execute_ddl` /
   `execute_ddl_with_deps` each recorded a `sql` field built by
   `sql_fingerprint`, which returned the statement VERBATIM at 440 characters
   or shorter.
2. **Log lines above `DbHandle`.** `MatviewManager::ensure_view` logged the
   whole `CREATE MATERIALIZED VIEW … AS <statement>` at DEBUG and `preload`
   logged it again at WARN on failure; `prime_fdw_caches`,
   `DynamicSchemaModule::ensure_schema`, `McpSyncEngine` and the
   `[actor-stats sql]` histogram each logged a raw statement at INFO.
3. **Operation parameters, not SQL at all.**
   `[BackendEngine] execute_operation` and
   `[OperationDispatcher] execute_operation` printed the whole parameter bag
   with `{:?}` at INFO, so every write logged its own content and property
   values before any statement existed. The GPUI search overlay recorded the
   user's raw search term as a span field (`frontends/gpui/src/search_ui.rs`).

Two panic classes travelled with them: `&sql[..120]` and `&sql[..200]` are BYTE
slices, so a multi-byte character astride the boundary panics — and user
content is what puts a character at that offset. Measured in round 1:
`lane-logs/red-01.log`, `end byte index 120 is not a char boundary`.

## Root cause

`sql_fingerprint` (crates/holon-turso/src/turso.rs) was built as a dedup bucket
for the PBT N+1 reporter, where readable statement text is the point. It was
then reused as a span field, and copied as an idiom — "log the statement, trim
it if it is long" — into every layer that wanted to say which statement it was
working on. One call site already knew better (`actor_watch.rs` wrapped it in
`blank_comments_and_string_literals`), and `docs/Architecture/Storage.md`
recorded the asymmetry as a known fact rather than as a defect: "The span
prefix is not redacted".

The parameter-bag lines have the same shape of cause: `{:?}` on a
`StorageEntity` is the cheapest way to make a log line informative, and nothing
distinguished the parameter NAMES (schema, safe) from their VALUES (content).

## Missing piece

No invariant asserted that an exported span field, log line or error message is
free of user values. The keystone PBT writes blocks through this whole stack, so
the interaction was fully generatable — nothing would have gone red.

Round 1's fix came with an oracle
(`crates/holon-turso/tests/sql_log_fields_carry_no_literals.rs`) and claimed
"no SQL statement text with user data reaches any span, log line or error in
holon-turso or holon". **The verifier refuted that claim**
(`lane-logs/verify-sec-state.md`): the oracle drove `DbHandle` and watched only
the `holon_turso` target, and `DbHandle` is the LAST seam a statement crosses.
Every leak above it — `MatviewManager`, `preload`, the dispatcher's parameter
bag, fifteen error and panic messages — ran in no part of that test, so the
oracle could not have gone red for any of them. The gap is ORACLE in what it
asserted and ENVIRONMENT in what it ran: a one-seam harness cannot observe the
layers that call it.

## Remedy

- `turso::redact_sql_for_logs` is the single form a statement may take in a
  span field, a log line or a message: comments, string literals and numeric
  literals blanked, then truncated with an identity hash taken over the BLANKED
  text (a hash of the original would be a crackable oracle over the blanked
  value for anyone holding the log). `sql_fingerprint` and the blanker are
  private, so no other module can log a raw statement.
- The blanker now takes `QuotedSpans`. The write guards read a target
  identifier back out of the blanked text and need `"t"` intact; a LOG cannot,
  because SQLite reads `"x"` as a string literal whenever `x` resolves to no
  column. An unterminated quote of any kind blanks to the end of the statement
  — the opposite reading left every value behind a stray quote in the log.
- Sites routed through it: the four `DbHandle` instrument sites and both DDL
  previews, both REPLACE refusals, the ungated actor statement trace,
  `sql_parser`, `turso_adapter`, `dynamic_schema_module`, `turso_actor_stats`
  (inside `fingerprint_sql`, so the histogram key is redacted too), nine
  `matview_manager` sites (`ensure_view`'s CREATE log and its parse context,
  `preload`'s failure WARN, the create-failure context, both clock refusals,
  the ORDER BY re-apply context, and both FDW priming sites), the two
  `[query: …]` row-conversion errors in `turso.rs`, `guard_world`,
  `di::registration`, `McpSyncEngine`, and the six `BackendEngine` sites.
- `api::param_keys_for_logs` replaces the two `{:?}` parameter bags with the
  sorted parameter NAMES. The GPUI search overlay logs the term's LENGTH.
- Every byte slice of a statement is gone; the redacted fingerprint truncates
  by character.
- Pinned END TO END by
  `crates/holon/tests/no_user_value_reaches_a_log_span_or_error.rs`: it drives a
  real `BackendEngine` (a write carrying a sentinel string and a sentinel number
  in content and properties, a watched query, a failing view creation, a refused
  clock watch, a failing query) while capturing EVERY target at TRACE plus every
  error `Display` returned to the caller, and asserts no sentinel appears
  anywhere — and that the statement SHAPE still does, so a blanket blanking
  cannot pass it.
  `crates/holon-turso/tests/sql_log_fields_carry_no_literals.rs` and the unit
  tests in `turso.rs` keep the seam-level and blanker-level properties.
- Both parameter fingerprints digest with a KEYED hasher whose key is drawn at
  random once per process (D60.a). A fingerprint is a digest of user data in a
  log, so an unkeyed one let anyone holding the log confirm a guessed value. The
  key costs cross-run comparability, which nothing uses.
- Red/green/teeth: `lane-logs/r2-red-03.log` (red on the round-1 tree, naming
  `ensure_view`'s CREATE log, `preload`'s failure WARN and both parameter bags),
  `lane-logs/r2-green-02.log`, `lane-logs/r2-teeth-four-sites-inverted.log`
  (four sites inverted, every resulting line caught; files restored
  byte-identical, `lane-logs/r2-teeth-sha-before.txt` =
  `lane-logs/r2-teeth-sha-after.txt`).

Kept with a reason, not redacted:

- `db_open.rs` (the scalar-function signature table) builds its statements from
  a `const` table name and binds the one value it writes, so its `{sql}: {e}`
  messages carry no user data. The `#[cfg(test)]` sites there, in
  `turso_adapter.rs` and in `clock_scheduler.rs` interpolate SQL literals
  written in the test file itself.
- `HOLON_TRACE_SQL` logs statements with their values intact by design —
  `turso-sql-replay` cannot replay a redacted statement. Documented at the
  function; it is an explicit developer opt-in.

Still open, reported rather than fixed:

- `turso_core` logs the statement and each string value of its compiled program
  at DEBUG; `sqlparser` logs every statement it parses and each string literal
  it reads at DEBUG. Both are dependencies' own code, and the production filter
  leaves both at the default ERROR, so only an explicit `RUST_LOG` turns them
  on. The end-to-end oracle excludes exactly these two targets by name, which
  is the one hole in it: a developer who runs with `RUST_LOG=debug` gets the
  values back.
- Blanks preserve each value's WIDTH, so a value's length is still disclosed.
  That is needed to keep byte offsets for the REPLACE guard, and is ruled
  acceptable (D61.b).
