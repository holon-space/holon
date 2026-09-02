---
id: 2026-09-02-toggling-a-slot-born-block-re-reads-its-task-state-89-times
date: 2026-09-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Toggling the task state of a block born through the creation slot re-executes
  one identical `task_state` SELECT 89 times for a single keystroke, 25 over the
  redundancy ratchet of 64.
---

## Bug

`just pbt general 16` (the composed keystone at its default case count) fails
with `inv-sql-budget` as the sole violation:

```
[inv-sql-budget] ToggleState.sql_read_repeat: one binding-set of `SELECT json_extract(properties, '$.task_state') AS task_state FROM block_raw WHERE id = 'b…` re-executed 89x, over the redundancy ratchet 64 — the re-execution defect GREW; find the new consumer, do not raise the ratchet
```

Found by the caret-attribution lane while running the keystone as the
post-fix gate for an unrelated oracle fix
([[2026-09-02-slot-birth-leaves-a-stale-ref-editor-that-suppresses-the-chord-click]]).
The run reported 31 novel panics, which are this ONE signature plus its 30-deep
shrink tail.

This is a real product defect, not a harness artefact: 89 identical reads of
one row answer a single state toggle. The ratchet exists precisely to catch the
re-execution roster growing, and the assertion text says what to do — find the
new consumer, do not raise the ratchet.

## Reproducer

Deterministic, TWO transitions, sole violation. The shrunk keystone case was
`BulkExternalAdd → CreateBlockUnderFocus → ToggleState`; the bulk add is not
needed.

```json
{"name": "toggle-done-on-slot-born-block", "initial_state": {"wiring": {"storage_adapters": ["Turso"], "sync_adapters": [], "actors": []}}, "transitions": [{"CreateBlockUnderFocus": {"content": "a", "id": null}}, {"ToggleState": {"block_id": "block::create-0", "new_state": "DONE"}}]}
```

Replay by putting that line in a sidecar:

```
HOLON_HAND_AUTHORED_SIDECAR=<abs path> \
HOLON_HAND_AUTHORED_CASE=toggle-done-on-slot-born-block \
cargo test -p holon-integration-tests --features pbt --test hand_authored_regressions -- --nocapture
```

NOT added to `hand-authored-regressions/keystone.jsonl`: it fires red, and
committing it would turn the `hand-authored` land gate red. Pin it RED-FIRST
when the fix is taken up.

## Attribution

Pre-existing on `main`, byte-identical repeat count on both trees:

| tree | rev | result |
|---|---|---|
| main baseline | `50f878cc3824` | RED, `re-executed 89x` |
| integration tip (+ subtree-share-race + the caret oracle fix) | `a7452468` + working copy | RED, `re-executed 89x` |

Logs: `main-baseline/lane-logs/toggle-probe-main-1.log`,
`subtree-share-race/lane-logs/toggle-probe-tip-1.log`. The ratchet constant
`MAX_READ_REPEAT_PER_BINDING = 64` is identical in both trees
(`crates/holon-integration-tests/src/pbt/transition_budgets.rs:334`), so the
comparison is fair. The caret oracle fix cannot reach this path: `BulkExternalAdd`
never touches `active_editor`, so at the birth in that sequence the field is
already `None` and both halves of that change are no-ops.

## Root cause

NOT ATTRIBUTED — the consumer that issues the repeats is not yet identified,
and the assertion deliberately refuses to guess. What is established:

- The read is `SELECT json_extract(properties, '$.task_state') AS task_state
  FROM block_raw WHERE id = $id`, one binding-set, i.e. the SAME row 89 times.
- It reproduces in a bare `storage={Turso} sync={} actors={}` draw, so neither
  Loro, nor org write-back, nor the MCP actor is required.
- The block is one born through the CREATION SLOT (`id: None`,
  `birth_block_under_slot`). Whether a slot-born block is required or merely
  the shortest path to a toggleable block is untested — the obvious next probe
  is the same toggle against a seeded id such as `block:parent`.
- Telemetry from the keystone draws: `ToggleState: reads=151..153 (dedup 25/26)`,
  writes 5-10, wall ~1200ms. The wall time alone is 6× the 200ms
  interaction→visible SLO, so this is latency-relevant, not only read-count
  hygiene.

## Missing piece

The oracle is not what failed here — `inv-sql-budget` fires correctly and names
the defect precisely. What let it into `main` is exploration budget: the
land battery's blocking keystone leg is `keystone-smoke`, which is
`PROPTEST_CASES=1`, a single draw. The sequence that triggers this needs a
creation-slot birth followed by a `ToggleState` on that same newborn, which one
draw almost never produces. The 16-case leg that does find it is not in the
blocking path.

Classification note: none of the four gaps fits cleanly. It is not ORACLE (the
invariant exists and fires), not ENVIRONMENT (it reproduces in the plainest
wiring the keystone draws), and not PERCEPTION. COVERAGE is the closest and is
recorded here in the generation sense — the catalog CAN produce the sequence,
but the gate's case budget means it effectively never does, which is the same
failure mode as a narrowed alphabet.

Measured draw rate in this lane, for whoever picks it up: across five
`keystone-smoke` runs (two before the caret fix, three after) it appeared ZERO
times; the single 16-case run hit it on its first draw and died there
(`WARN: low-power run (1 draws)`). A parallel 16-case run on `main` never drew
it and got through 45 known-red panics instead. So smoke-green is no evidence
about this family.

## Remedy

OPEN. Not fixed, and not this lane's to fix — it belongs to the `inv-sql-budget`
/ redundant-read roster (task #15).

Note for the next land battery: this signature is UNREGISTERED in
`docs/Testing/KeystoneKnownReds.md`, and `scripts/keystone-known-reds.sh`
classifies only rows whose status is `known-red`. The classifier will therefore
keep reporting it as NOVEL on any keystone run that draws it. Registering it
would silence a live, uncharacterised product defect, so the entry stands here
instead until someone triages the consumer.
