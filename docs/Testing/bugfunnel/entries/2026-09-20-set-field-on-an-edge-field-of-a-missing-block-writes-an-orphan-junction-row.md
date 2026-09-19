---
id: 2026-09-20-set-field-on-an-edge-field-of-a-missing-block-writes-an-orphan-junction-row
date: 2026-09-20
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  `set_field` on an EDGE field of a block that does not exist reports success
  and writes a junction row whose source block is a ghost, so the edge outlives
  a subject that was never there.
---

## Bug

Found by the VERIFIER of lane `set-field-assert` while probing the D147.a
changed-row assert for holes (`lane-logs/set-field-assert-verify.md`, probe log
`lane-logs/verify-03-probes.log`). Driving the SQL provider with
`set_field("requires", [block:anchor])` on `block:ghost` — an id no row carries
— returns

```
PROBE edge-field-on-ghost = Ok(OperationResult { changes: [], ... })
PROBE edge-field-on-ghost junction rows = 1
PROBE orphan row = {"block_id": "block:ghost", "required_id": "block:anchor"}
```

The caller is told its edge write landed, and the junction now holds a row
whose source block does not exist. This is worse than the row-backed case D147.a
just closed: there the write was a silent no-op, here it silently CREATES
state. The orphan is reachable by every reader of the junction and nothing
will ever collect it, because nothing owns a block that was never born.

## Root cause

`SqlOperationProvider`'s single-op `set_field` arm handles an edge field in its
own early-returning branch (`crates/holon/src/core/sql_operation_provider.rs`,
the `if let Some(descriptor) = self.edge_fields.get(field)` block). That branch
captures the current targets, then runs `edge_field_replace_sql` — a DELETE of
the source's existing rows followed by an INSERT per target — and returns.

It never touches the entity row. So the assert D147.a added,
`assert_row_matched`, has nothing to read: the count it reads is the count of
entity rows the UPDATE changed, and this branch issues no UPDATE. The DELETE
legitimately matches zero rows (a ghost has no edges), and the INSERT
legitimately writes one, so no statement in the branch reports anything
anomalous. There is no FK to catch it either — the junction's source column
carries no foreign key to the block table.

This is a genuinely different fix from D147.a, which is why that ruling did not
cover it: the row count is not the signal. The signal has to be either a
foreign key on the junction's source column (declarative, catches every writer,
but the deferred-FK-at-COMMIT behaviour needs the transaction discipline the
`turso-fk-autocommit-wart` note describes) or an explicit subject-existence
read before the replace (one SELECT on a path that already issues several
statements, so the latency argument that shaped D147.a is weaker here).

## Missing piece

**ORACLE.** Invariant 15 in `docs/Architecture/Model.md` states that a
dispatched operation targets an existing subject and that the write authority
enforces it. No property asserts it for the edge branch, and the invariant's
own headline had to be qualified to stop over-claiming while this is open.

**Secondary COVERAGE.** `set_field_missing_subject_test.rs` drives the column,
property-bag, `parent_id` and rich-content legs against a ghost. It does not
drive the edge leg, because that leg needs a junction table and an edge-field
descriptor in the fixture, which the row-backed tests do not set up.

## Remedy

OPEN. Deliberately not fixed in lane `set-field-assert`: D147.a ruled on the
changed-row assert, and both candidate fixes here are decisions Martin has not
made.

1. **Foreign key on the junction's source column.** Declarative and catches
   every writer of the junction, not only this branch. Needs the reparent
   transaction discipline (deferred FK is checked at COMMIT, and autocommit
   leaves the bad row written despite the raised error) extended to the edge
   branch, and a schema migration for the existing junctions.
2. **Subject-existence read before the replace.** Local to this branch and
   symmetric with the batch path's `assert_updated_rows_exist`. Costs one
   SELECT, on a path that is not the keystroke path the p95 < 200ms SLO governs
   and that already issues a capture plus N statements — so the latency
   objection that ruled out a pre-read in D147.a does not transfer unexamined.

Red-first either way: extend `set_field_missing_subject_test.rs` with an
edge-field fixture (the junction table plus an `EdgeFieldDescriptor`) and
demand `Err` plus zero junction rows.

## Relation to other entries

The row-backed half of this defect is
`2026-09-19-set-field-on-missing-block-succeeds-silently`, FIXED by D147.a.
This entry is the leg that fix structurally cannot reach, and it is linked from
invariant 15 as the named exception.
