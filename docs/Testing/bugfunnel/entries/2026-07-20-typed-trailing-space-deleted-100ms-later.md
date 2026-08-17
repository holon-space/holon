---
id: 2026-07-20-typed-trailing-space-deleted-100ms-later
date: 2026-07-20
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Typed trailing space DELETED ~100ms later while typing (user report,
  desktop, SqlOnly/no-cell blocks; intermittent — only when the space stays
  last char until the echo lands): editor dispatches `set_field content="foo
  "` stamped `write_seq=N` (`editor_view.rs:291,329-375`);
  `SqlOperationProvider::trimmed_content` stores `"foo"` WITH the same
  `write_seq=N` (`sql_operation_provider.rs:329-343,418`); CDC echo `("foo",
  seq=N)` hits `evaluate_data_sync_echo` whose `>=` guard
  (`editor_view.rs:87-90`) cannot distinguish a newer external authority from
  the SQL-canonicalized echo of the user's OWN in-flight keystroke →
  `converge_input` `set_value("foo")` overwrites the focused buffer. ~100ms =
  write→matview/CDC→signal round-trip (no timer). Loro/cell blocks unaffected
  (converge to untrimmed `cell.current()`); org-writeback trim echo correctly
  dropped (re-ingest `write_seq=0` → DropStale). Refines row 243 (the
  projection-trim observation is this bug's substrate; NOT by-design when it
  regresses a focused buffer).
source_line: 1042
---

## Bug

Typed trailing space DELETED ~100ms later while typing (user report,
desktop, SqlOnly/no-cell blocks; intermittent — only when the space stays
last char until the echo lands): editor dispatches `set_field content="foo
"` stamped `write_seq=N` (`editor_view.rs:291,329-375`);
`SqlOperationProvider::trimmed_content` stores `"foo"` WITH the same
`write_seq=N` (`sql_operation_provider.rs:329-343,418`); CDC echo `("foo",
seq=N)` hits `evaluate_data_sync_echo` whose `>=` guard
(`editor_view.rs:87-90`) cannot distinguish a newer external authority from
the SQL-canonicalized echo of the user's OWN in-flight keystroke →
`converge_input` `set_value("foo")` overwrites the focused buffer. ~100ms =
write→matview/CDC→signal round-trip (no timer). Loro/cell blocks unaffected
(converge to untrimmed `cell.current()`); org-writeback trim echo correctly
dropped (re-ingest `write_seq=0` → DropStale). Refines row 243 (the
projection-trim observation is this bug's substrate; NOT by-design when it
regresses a focused buffer).

## Missing piece

ENV: the failing composition (GPUI async `_data_subscription` +
`converge_input` vs a focused live `InputState`) is not instantiated
headless; keystone model mirrors the trim (`types.rs:139` ← doc-ref
`reference_state.rs:622-628`) and settles at quiescence, masking the
transient. ORACLE: no invariant "a focused editor buffer is never regressed
by the canonicalized echo of its own same-seq write". Remedy: strict-`>`
converge (same-seq echo adopts baseline silently, never `set_value` while
focused) + windowed rung typing trailing space through one CDC round-trip;
longer term move buffer+seq+echo policy into `EditorViewModel` so the
invariant runs headless.

## Remedy

FIXED 2026-07-20 (EchoDecision::AdoptBaseline + shared
holon_api::content_canonical discriminator; windowed rung 3/3 red->green;
verifier CONFIRMED x2)
