---
id: 2026-09-15-turso-view-change-deleted-carries-the-entity-id
date: 2026-09-15
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Under contention the `entity_view` change stream reports a `Deleted` change
  carrying the ENTITY id where the rowid is expected, so a consumer that keys
  events by rowid resolves `None` and sees a deletion it cannot match.
---

## Bug

`holon::turso_storage_pbt pbt_tests::tests::test_turso_backend_state_machine`
failed during a CONTENDED nextest run and is green in isolation. Captured
firing: `.claude/worktrees/docs-w14/lane-logs/turso-state-machine-contended-1789429950.log`,
whose header records the conditions verbatim — "Captured contended run …,
integration lqxuknonwyyq `30e2bc43`, 2026-09-15 ~03:05, inside a 2149-test
nextest run (gate-entity-uri). Isolated 3/3 green afterwards on the same tree."

The assertion at `crates/holon/tests/turso_storage_pbt/pbt_tests.rs:1988` compares
the change the view stream reported against the change the reference modelled.
The stream reported the ENTITY id where the reference expected the ROWID:

```
    === View Change Entity ID Mismatch at index 4 for 'entity_view' ===
    Expected entity ID: Some("xui")
    Actual entity ID: None
    Expected change: Deleted { id: "2", origin: Remote { … } }
    Actual change: Deleted { id: "xui", origin: Remote { … } }
```

The sequence that reaches it is the one the log's last drawn case carries:
`CreateMaterializedView(entity_view)` … `Delete("nepo")` + `Insert("xui")` …
`Update("xui")` … `Insert("zzhzx", parent "xui")` … `Delete("xui")`. The mismatch
fires on the deleted-row event and repeats 22x in the 522-line excerpt, on every
replay of the same case.

This is the same signature that was first appended to
`2026-09-01-holon-crate-integration-tests-ungated` under "Captured contended run
2026-09-15". It gets its own id here because that entry's defect is the missing
`-p holon` gate, not this one, and a defect recorded only as a section of an
unrelated entry has no handle to cite and cannot be counted on its own.

Sibling defect, different mechanism, in the same test binary:
`2026-09-15-turso-query-bind-index-out-of-bounds-after-matview`.

## Root cause

UNATTRIBUTED. The observable is that a consumer keying change events by rowid is
handed the entity id instead and resolves `None`, i.e. it sees a deletion it
cannot match to a row it holds. Nothing in the capture names the mechanism, so
what follows is the shape of the problem, not a located cause.

`Remote { operation_id: None, trace_id: None }` on the actual change says the
event arrived from the CDC leg with no operation provenance attached, which is
consistent with a projection-leg emission that did not go through the writer
that stamps origin — but the excerpt does not distinguish that from an event
whose provenance was dropped in transit.

## Missing piece

The test environment is single-threaded per case and settles between
transitions, so the interleaving that produces this never arises. The
interaction IS generatable — the case above is drawn from the ordinary alphabet
— but the timing that breaks it is not. That is what makes this ENVIRONMENT
rather than COVERAGE: the keystone reaches the state and the assertion holds
every time it runs the way the test runs it.

The ORACLE secondary is that no invariant expresses "every change a view stream
emits keys to the same identity the reference does" outside this one unit
assertion, so a consumer-side identity confusion is invisible everywhere except
this binary, under load.

## Remedy

OPEN. The gating decision in
`2026-09-01-holon-crate-integration-tests-ungated` is unaffected. Attribution
needs a dedicated lane: run the case concurrently in-process (two draws, shared
store) and see whether the rowid/entity-id swap reproduces, then follow the
`Deleted` emission path in the view-stream projection. Until then this is a
prod-bug candidate of the CDC kind, not a test defect — the assertion is
comparing the right things and the reference is right.
