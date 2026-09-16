---
id: 2026-09-15-turso-query-bind-index-out-of-bounds-after-matview
date: 2026-09-15
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  A `Query` carrying an `Or` filter fails with `bind index 1 is out of bounds`
  once a materialized view exists in the same case, and only under contention —
  the same case is green in isolation and green in most contended runs.
---

## Bug

`holon::turso_storage_pbt pbt_tests::tests::test_turso_backend_state_machine`
panicked at `crates/holon/tests/turso_storage_pbt/pbt_tests.rs:1778:30`:

```
    Turso transition should succeed (preconditions validated it):
    "Query error: Failed to execute query: bind index 1 is out of bounds"
```

Captured at `.claude/worktrees/entity-uri-boundary/lane-logs/h-g2-holon-1789486742.log`,
where the string occurs 45x. The drawn case that reaches it:

```
    2/11: Insert p qqzcc, g tplb, wtet dryhdsuud          (Concurrent)
    4/11: Query And([Eq("id","dryhdsuud"), Eq("parent_id","dryhdsuud")])
          + CreateMaterializedView { view_name: "entity_view" }   (Concurrent)
    6/11: Insert uhd, t (parent p), pj, lr                  (Concurrent)
    7/11: Delete g + Get p                                  (Concurrent)
    8/11: Query Or([IsNotNull("value"),
                    And([Eq("value", String("wtet")), IsNotNull("value")])])   ← panics
```

`bind index 1 is out of bounds` is a placeholder with no parameter bound behind
it. The `Or` filter compiles to exactly one `Eq` — one placeholder — so a
statement reaching the engine with one placeholder and zero bindings is the
shape, not a count mismatch inside the filter.

Contention-only, established by measurement rather than by the label:

| run | shape | this test |
|---|---|---|
| `h-g2-holon-1789486742.log` | 564 tests, 59 binaries, parallel | FAIL, 45 panic lines |
| `h-g2b-holon-1789487671.log:661` | same 564-test leg | PASS |
| `h-flake-holon3-1789488256.log:635` | same 564-test leg | PASS |
| `h-ab-{base,mine}-run{1,2,3}-1789487630.log` | isolated, whole binary | PASS 6/6, each `2 tests run: 2 passed` |

Two of three contended runs of the same leg pass it, and the isolated binary is
green three times on the base tree and three times on the lane tree, so this is
neither a lane regression nor a deterministic red. The failing run closed
`Summary [ 179.587s] 564 tests run: 557 passed (3 slow), 7 failed, 9 skipped`;
its other six failures are the registered
`holon::e2e_backend_engine_test` set plus `capability_certification`, so the run
also carried the load signature the known-reds registry already documents.

Sibling defect, different mechanism, in the same test binary:
`2026-09-15-turso-view-change-deleted-carries-the-entity-id`.

## Root cause

UNATTRIBUTED. What the capture fixes is the trigger shape: an `Or` filter whose
`Eq` arm is bound while a materialized view exists in the same case. Two
readings fit the evidence and the log does not separate them —

1. the matview's incremental-maintenance statement is prepared with the
   user query's placeholder layout and executed with the wrong binding set, so
   the maintenance write is what raises; or
2. the user query is re-planned after `CreateMaterializedView` and the re-plan
   emits a placeholder the original binding pass never saw.

Reading 1 is the more likely of the two, because the failure needs the matview
to exist and needs contention, and maintenance is the only part of the path
that is asynchronous. It is a hypothesis, not a finding.

## Missing piece

The test draws and applies one transition at a time and settles between them, so
no maintenance statement ever runs concurrently with a query in the test's
environment. The interaction is generatable — it is drawn from the ordinary
alphabet, and the case above is one the generator produced without help — but
the concurrency that breaks it exists only when the whole 564-test leg runs in
parallel. ENVIRONMENT rather than COVERAGE.

The ORACLE secondary: no invariant asserts that a statement reaching the engine
has every placeholder bound, so a mis-bound statement can only surface as the
engine's own error text in whichever test happens to execute it.

## Remedy

OPEN. Two things are separable and only the first is cheap: find whether the
maintenance path builds its own statement text, and if it does, assert at that
seam that the placeholder count matches the bound parameters before execution —
a fail-loud guard turns a contention-only mystery into a located error the next
time it fires. The engine-side question (why a re-plan changes the binding
count) needs the dedicated triage lane the sibling entry also asks for.
