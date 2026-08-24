---
id: 2026-08-24-task68-loro-fileless-page-flake
date: 2026-08-24
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  `inv-every-page-has-its-own-file` reds intermittently on the hand-authored
  case `task68-uppercase-nonkeyword-commits-verbatim-loro`, reporting a
  deterministic seeded page as fileless — observed 1 run in 4, only on a heavily
  loaded machine.
---

## Bug

Seen at a landing gate (`just hand-authored` at tip `ab0f022c`), not by a test
author and not by dogfooding, so it has no known-red row to fall under.

```
inv-every-page-has-its-own-file: 1 page(s) not homed to exactly one own file:
  page `61133fe7-d4d5-ab1c-24e4-110da5f42293` owns NO file (fileless — content
  lives only in the store; writeback must MATERIALIZE it into
  `…/61133fe7-d4d5-ab1c-24e4-110da5f42293.org`)
authority: block-CRUD=Loro(LoroBlockOperations, via EditorState);
projection-sinks=Sql(block_raw,matview); org-writeback=on
```

The `-sqlonly` twin passed, which is wiring and not evidence about the defect:
that twin declares `storage_adapters: ["Turso"]` with no `Org` leg, so the
file-homing invariant has nothing to assert.

**Observed rate 1 in 4.** Red once at the gate; green in three consecutive
reproductions on the identical tree (`.lane-logs/ha-repro.log`,
`ha-repro2.log`, `ha-repro3.log` — 9 passed / 0 failed at 530.75s, 485.36s,
461.11s; runs 2 and 3 name the case `PASSED` explicitly).

## Root cause

**Not established.** What is established:

**The subject is deterministic.** `61133fe7-d4d5-ab1c-24e4-110da5f42293` is a
seeded page the replay always creates (`RenamePage` at transition 1/14,
`BulkExternalAdd` into it at 15/22) and it appears in all three green runs. Same
entity, same transitions, file present three times and absent once — so the
difference is not data shape, id minting, or case selection.

**A race site exists.** The org writeback declines to materialize while a
reconciling diff is outstanding, and says so:

```
WARN holon_filesystem::file_sync_controller: [FileSyncController] write-back
SKIPPED: the holder's membership does not match the authority's, so this render
would project a partially-folded document over disk. The diff that resolves it
is already in flight
```

If the invariant samples while that deferral holds a page, the page is fileless
exactly as reported. This is the mechanism the report proposes.

**Counter-evidence against the obvious form of that hypothesis — recorded
because it is what the data says.** Deferral VOLUME anti-correlates with the
failure: the red run logged 82 `write-back SKIPPED` lines, the three green runs
122 / 139 / 130. So "more deferrals ⇒ fileless page" is false. The skip is
common and benign in green runs; at most it locates where a fileless
observation can come from, not why this one happened.

**Load correlates, weakly.** Red run: 798.84s wall, 102 `over the p95 SLO`
warnings, including `settle_budget` / `TypeChars` at 501ms against a 200ms SLO
immediately before the failing case. Green runs: 461–531s, 69 / 69 / 70
warnings. Suggestive, not causal.

Whether the deferred write eventually lands (making this an oracle sampling
defect) or can be dropped (making it a real writeback defect) is the open
question, and it is what the next probe must decide.

## Missing piece

Nothing establishes that the harness reaches org-writeback QUIESCENCE before
`inv-every-page-has-its-own-file` samples. The invariant asserts a
materialization that the writeback layer is explicitly allowed to defer, and no
barrier relates the two. That is why it is filed **ORACLE**: the invariant may
be reading a legitimately-intermediate state and calling it a violation.

**ENVIRONMENT** secondary because whether it reads that state at all depends on
machine load, which is why it survived every quiet-machine run including the
one that gated D5.a an hour earlier.

The classification is an argument, not a measurement: if the probe below shows
the deferred write never lands, this is not an oracle gap at all but a genuine
writeback defect, and the entry should be re-classified rather than closed.

## Remedy

**OPEN.** Not fixed, and deliberately not "fixed" by retry.

Next probe, in order:
1. Loop `task68-uppercase-nonkeyword-commits-verbatim-loro` alone under
   synthetic load and record the rate — a cheap way to turn 1-in-4 into a
   signal.
2. On a red, check whether the page's file appears if the harness waits: that
   single observation separates "sampled too early" (oracle) from "the write
   was dropped" (a real defect, and a much more serious one).
3. If oracle: give the invariant a writeback-quiescence barrier, in
   `crates/holon-integration-tests/src/pbt/composed/invariants/every_page_has_its_own_file.rs`,
   whose owner should take this.

Two notes on where this belongs. It was found by an automated test rather than
outside one, so it sits at the edge of this ledger's scope; it is filed here
because the thing being classified — the invariant's sampling discipline — is
itself uncovered. It should probably ALSO get a row in
`docs/Testing/KeystoneKnownReds.md` so the next lane meeting this signature can
recognize it; that registry is not this entry's to edit.

Attribution checked and excluded: the tip commit that met this red
(`ab0f022c`, connector entity-name canonicalization) cannot execute in this
harness — `register_fake_mcp` has two callers, both in `test_environment.rs`
behind `enable_fake_mcp`, and the composed harness references neither it nor
`McpIntegrationsModule`, which is registered only from `wiring.rs:353
from_dir` and finds no configs here.
