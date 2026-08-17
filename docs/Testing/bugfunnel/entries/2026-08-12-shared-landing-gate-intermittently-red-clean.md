---
id: 2026-08-12-shared-landing-gate-intermittently-red-clean
date: 2026-08-12
gap: ENVIRONMENT
secondary: null
status: UNCLASSIFIED
summary: >-
  `just hand-authored`, a SHARED LANDING GATE, is intermittently red at
  ~1-in-4 on CLEAN BASE, and the failures are DISTINCT races, so a single
  green run is not evidence the tree is good.
source_line: 1200
---

## Bug

(LAT.a lane, found by REPLICATION while splitting a lane for landing — not
by any single test verdict) **`just hand-authored`, a SHARED LANDING GATE,
is intermittently red at ~1-in-4 on CLEAN BASE, and the failures are
DISTINCT races, so a single green run is not evidence the tree is good.**
Measured, same machine, same day, 8 full runs of `cargo test -p
holon-integration-tests --features pbt --test hand_authored_regressions`
(`HOLON_PERF_BUDGET=1`): BASE (`1256a144`, no lane changes) **1 red / 4**;
the lane tree (loud bare-focus fallback + per-database matview sharing) **1
red / 4**. Identical rate, DIFFERENT signatures: (a) BASE `base-1.log` —
`mutation_driver.rs:495` `[DirectUserDriver floor]
block/convert_block_to_page failed: … constituent 'move_block' failed:
Parent not found`; (b) lane `A-hand-authored.log` — a LOST BLOCK in case
`instantiate-template-undo-removes-whole-instantiation`: after
`InstantiateTemplate{parent: block:cu-host, date: "abc", mood: "xyz"}` the
instantiated grandchild `block:4771e01b-…` (content "see abc now") is
present in the reference and ABSENT from Loro, `block_raw`, matview, org and
the panel, with the harness attributing `first-divergent-layer: store/CRDT …
nothing below it`, `[gen-drop] no generation-guard drops recorded`,
`[tree-desync] no tree/row_map divergence recorded`. Both reds are the FAST
runs (173.76s and 219.66s) against 337–613s for greens, i.e. they surface on
the LESS contended machine — direction noted, mechanism NOT established. The
lane change was initially and WRONGLY reported as the cause of (b) on n=1;
replication refuted that (isolated case 3/3 green, full suite 3/4 green, and
base reds at the same rate).

## Root cause

LAT.a lane, found by REPLICATION while splitting a lane for landing — no
single test verdict produced it: **`just hand-authored`, a SHARED LANDING
GATE, is intermittently red at ~1-in-4 on CLEAN BASE, and the failures are
DISTINCT races, so one green run is not evidence the tree is good and one
red run is not a regression signal.** Measured, same machine, same day, 8
full runs (`HOLON_PERF_BUDGET=1`): BASE (`1256a144`, no lane changes) 1 red
/ 4; the lane tree 1 red / 4 — identical rate, different signatures. (a)
BASE: `mutation_driver.rs:495` `[DirectUserDriver floor]
block/convert_block_to_page failed: … constituent 'move_block' failed:
Parent not found`. (b) LANE: a LOST BLOCK in
`instantiate-template-undo-removes-whole-instantiation` — after
`InstantiateTemplate{parent: block:cu-host, date: "abc", mood: "xyz"}` the
instantiated grandchild `block:4771e01b-…` ("see abc now") is in the
reference and ABSENT from Loro, `block_raw`, matview, org and the panel;
harness attribution `first-divergent-layer: store/CRDT … nothing below it`,
with `[gen-drop]` and `[tree-desync]` both empty. Both reds are the FAST
runs (173.76s, 219.66s) vs 337–613s for greens — they surface on the LESS
contended machine; direction noted, mechanism NOT established. The lane
change was initially and WRONGLY reported as the cause of (b) on n=1;
replication refuted it (isolated case 3/3 green, full suite 3/4 green, base
red at the same rate). ENVIRONMENT primary: the keystone drives real async
pipelines and the shared settle/quiescence barrier usually masks the
interleaving, so the same tree passes or fails run to run and no bisect can
attribute a red — the gap the skill names as "async races the settle masks".
Missing piece — already FUNDED and in flight: task #20 deterministic
scheduling (injectable spawner + per-kind staged interleaving), which turns
both signatures into reproducible cases. Until then the honest protocol for
this gate is N-run, not 1-run. NOT registered in `KeystoneKnownReds.md`: (b)
matches the existing `org-blocks-ref-diverge` pattern and (a) matches no
row; silencing live defects there is against that registry's own rule and
needs Martin. Evidence: `lane-logs/base-{1,2,3,4}.log`,
`full-A-{1,2,3}.log`, `A-hand-authored.log`, `det-A-{1,2,3}.log`. REPORTED —
not fixed by this lane.)

## Missing piece

The keystone drives real async pipelines and the shared settle/quiescence
barrier usually masks the interleaving; nothing in the harness can FORCE it,
so the same tree passes or fails run to run and no bisect can attribute a
red. This is the gap the skill names directly ("async races the settle
masks"). Remedy already FUNDED and in flight: **task #20 deterministic
scheduling (injectable spawner + per-kind staged interleaving)** — that is
what turns both signatures into reproducible cases instead of folklore.
Until it lands, the honest gate protocol is N-run, not 1-run: a single red
on this suite is not a regression signal and a single green is not a landing
signal. NOT registered in `KeystoneKnownReds.md`: signature (b) matches the
existing `org-blocks-ref-diverge` pattern (`diverged from the oracle:
.*"inv-blocks-match-ref/[a-z_]+".*fields diverge from reference`) and (a)
matches no row; adding rows to silence live defects is against that
registry's own rule and needs Martin. Evidence:
`lane-logs/base-{1,2,3,4}.log`, `lane-logs/full-A-{1,2,3}.log`,
`lane-logs/A-hand-authored.log`, `lane-logs/det-A-{1,2,3}.log`.

## Remedy

#20 | deterministic scheduling (task #20)
