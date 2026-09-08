---
id: 2026-09-08-a-cook-vaults-second-boot-re-parses-a-varying-number-of-recipes
date: 2026-09-08
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  The second boot of an unchanged three-recipe vault runs the wasm parser 4–7
  times instead of once, and the cold-boot skip's store-presence probe answers
  differently for the same document root across identical runs.
---

## Bug

Found while proving lowcode Inc 3's incremental scan (agent exploration, not a
test failure). The controller's cold-boot skip ends at
`content_present_in_all_stores(root)` — one `BlockOrdering::in_tree` call
(`crates/holon/src/core/sql_block_operations.rs:655` → `live_in_tree`). When it
answers `Some(false)` the skip does not fire and the file goes through the wasm
guest again.

Measured, not inferred: five identical runs of
`a_second_boot_re_parses_only_the_recipe_that_changed` (same tree, same vault,
same command; `lane-logs/rev3-probe5.sh`, logs `lane-logs/rev3-probe/run-1..5.log`,
extraction `lane-logs/rev3-probe/DISTRIBUTION.txt`):

| run | second-boot guest parses (expected 1) | `in_tree` Pancakes root `block:0790f49e-…` | `in_tree` Waffles root `block:7009cf16-…` |
|---|---|---|---|
| 1 | 6 | false | false |
| 2 | 4 | true  | true  |
| 3 | 5 | true  | false |
| 4 | 5 | true  | false |
| 5 | 7 | true  | false |

Two things follow, and they retire the earlier reading of this residual.

**The probe's answer is not stable.** The same root answers `false` in one run
and `true` in the next, and in runs 3–5 two roots in the SAME scan disagree.
So `in_tree` does not systematically mis-resolve a `.cook` document root; its
answer depends on when the probe runs relative to ingest.

**`in_tree` is not the only remaining cause.** In run 2 both probes answered
`true` — the skip had everything it needs — and the second boot still ran the
guest 4 times, not 1. Something re-parses beyond the roots this probe covers.

The org half of the comparison this entry previously asserted does not exist:
across all five runs there are exactly two `in_tree probe` lines per run, both
for `.cook` roots, and **zero for an org root**. The probe sits behind
`stored == &disk_hash` (`crates/holon-filesystem/src/file_sync_controller.rs:2898`)
and every org file in this vault had `stored != disk` on the second boot in
5/5 runs, so an org file never reaches the probe here. Org-vs-cook cannot be
compared in this harness at all.

## Root cause

Not located. Three candidates, each with the measurement that would separate it
from the others:

1. **Ingest/Loro settling race.** The Loro tree has not absorbed the document
   root when the probe runs. Discriminating measurement: log the tree's own
   membership for the same id immediately before and after the probe, in the
   same call — a false probe with the id absent from the snapshot is this; a
   false probe with the id present is (2).
2. **Resolution defect under concurrency** — `live_in_tree`'s stable-id → tree-node
   lookup reads a cell registry that a concurrent write is rebuilding.
   Discriminating measurement: the same probe on a QUIESCED app (boot, wait for
   every projection, then call `in_tree` directly through the MCP seam). A
   still-false answer with no concurrent writer rules out (1).
3. **A second parse leg that never consults the skip.** Run 2 shows 4 parses
   with both probes true. Discriminating measurement: a backtrace or span
   attribution per `GUEST_PARSES` increment
   (`crates/holon-plugin-host/src/adapter.rs:154`) — it names each caller, and
   only the ones inside `org.ingest_file` are the ones the skip governs.

## Missing piece

ENVIRONMENT. Nothing in the test wiring makes the ingest→tree-presence ordering
deterministic, and nothing fails when it lands the slow way: a `false` probe
degrades into "re-ingest", which looks like correct-but-slow behaviour. The
consequence is a latency escape (the wasm interpreter costs ~20x the native
parser it replaced) that no gate can see, and the run-to-run spread means any
before/after attribution on this counter needs repeats, not a single run.

Secondary ORACLE: `in_tree` has no test comparing its answer against the tree's
own contents for the same id — the one place both are observable — so a false
negative is invisible to every consumer that takes it at face value.

## Remedy

OPEN. Do NOT special-case the caller: scoping `content_present_in_all_stores`
past this answer for read-only formats would make the incremental scan green
while leaving every other `in_tree` consumer reading the same answer.

The covering test for the consequence is
`crates/holon-integration-tests/tests/cook_vault_ingest.rs::a_second_boot_re_parses_only_the_recipe_that_changed`,
`#[ignore]`d as a known red (measured 4–7 guest parses over five identical runs,
expected 1).
