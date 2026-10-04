---
id: 2026-10-04-sql-spans-record-raw-statement-literals
date: 2026-10-04
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Every DbHandle span and most SQL log lines recorded the statement verbatim, so
  note bodies and property values inlined into the SQL reached stdout, the
  HOLON_LOG file and the OTLP span exporter.
---

## Bug

`DbHandle::query` / `execute` / `execute_ddl` / `execute_ddl_with_deps` each
record a `sql` span field built by `sql_fingerprint`, which returns the
statement VERBATIM when it is 440 characters or shorter and head+tail with an
identity hash when longer. A statement reaches the database with its values
inline whenever the caller built the text instead of binding parameters, which
`BackendEngine::inline_parameters` does for every watched query. A note's body,
a property value or a search term therefore landed in the span field.

The production filter is `holon_gpui=info,holon=info,holon_tui=info`. EnvFilter
matches a directive target as a string prefix, so `holon=info` enables
`holon_turso::turso`, and the `#[tracing::instrument]` spans are INFO. The
fields were live by default, with no opt-in, on every destination the frontend
installs: stdout, `HOLON_LOG=file://…`, and the OTLP span exporter
(`crates/holon-frontend/src/logging.rs:348`), which sends them off the machine.

Found by a security audit of the lane `sec-sql-spans`, not by a test. A second
defect surfaced in the same code: `trace_sql` truncated with `&sql[..120]`, a
BYTE slice, so a multi-byte character astride byte 120 panicked the actor —
and user content is what puts a character at that offset. Measured:
`lane-logs/red-01.log`, `end byte index 120 is not a char boundary`.

## Root cause

`sql_fingerprint` (crates/holon-turso/src/turso.rs) was built as a dedup bucket
for the PBT N+1 reporter, where readable statement text is the point. It was
then reused as the span field without a redaction step. One call site already
knew better — `actor_watch.rs` wrapped it in
`blank_comments_and_string_literals` for the stuck-command report — and
`docs/Architecture/Storage.md` recorded the asymmetry as a known fact rather
than as a defect: "The span prefix is not redacted".

## Missing piece

No invariant anywhere asserts that an exported span field or log line is free
of statement values. The keystone PBT writes blocks through the same
`DbHandle`, so the interaction was fully generatable — nothing would have gone
red. The redaction helper existed; the property that every logging site must
use it did not.

## Remedy

- `turso::redact_sql_for_logs` is now the single form a statement may take in a
  span field, a log line or a refusal message: comments, string literals and
  numeric literals blanked, then truncated with an identity hash taken over the
  BLANKED text (a hash of the original would be a crackable oracle over the
  blanked value for anyone holding the log).
- Routed through it: the four `DbHandle` instrument sites, the
  `MissingDependencies` and dependency-timeout previews, both REPLACE refusal
  messages, the ungated actor statement trace, `sql_parser` and
  `matview_manager` warnings, and the six `BackendEngine` sites that log
  `sql_with_params`. `actor_watch::redact` now delegates to it.
- `trace_sql`'s byte slice is gone; the redacted fingerprint truncates by
  character.
- Pinned by `crates/holon-turso/tests/sql_log_fields_carry_no_literals.rs`
  (span fields and log lines carry no literal, and the statement SHAPE still
  survives) plus four unit tests in `turso.rs`. Teeth proved by inversion:
  `lane-logs/teeth-A-helper-inverted.log`,
  `lane-logs/teeth-B-query-site-inverted.log`.

Still open, reported rather than fixed:

- `HOLON_TRACE_SQL` logs statements with their values intact by design —
  `turso-sql-replay` cannot replay a redacted statement. Documented at the
  function; it is an explicit developer opt-in.
- `turso_core` logs the statement and each string value of its compiled program
  at DEBUG (`Preparing:`, `String8 { value: … }`). The production filter leaves
  `turso_core` at the default ERROR, so only an explicit `RUST_LOG` turns it
  on. It belongs to the engine fork.
- `named_params_fingerprint` / `positional_params_fingerprint` hash the bound
  values with `DefaultHasher`. That is a non-cryptographic hash of user data in
  the log, and it is the whole point of the field (telling one binding from
  another). Unchanged.
