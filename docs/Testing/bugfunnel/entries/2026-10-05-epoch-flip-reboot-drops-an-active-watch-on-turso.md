---
id: 2026-10-05-epoch-flip-reboot-drops-an-active-watch-on-turso
date: 2026-10-05
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  With the keystone wiring pinned to Turso, SetupWatch followed by
  EpochFlipRejected leaves the watch missing on the SUT while the reference
  model still holds it (inv-active-watches-match-ref).
---

## Bug
Found by a lane's keystone gate under `HOLON_PBT_PIN_WIRING="Turso;;"` (json-prop-boot lane,
`lane-logs/sas-r2-gate-ks-turso.log:30065`). The shrunk input is
`[SetupWatch(query-a), EpochFlipRejected]` (query-a: Blocks, columns id/content/content_type/
source_language/source_name/parent_id, AllBlocks, holon_prql). Verdict:
`[inv-active-watches-match-ref] watch sets diverged  missing on SUT: ["query-a"]  spurious on SUT: []`.
It reproduces 2 of 2 as a temporary hand-authored case on main b2a19917 source and tests
(`lane-logs/sas-r2-ab2-main-1.log:150`, `sas-r2-ab2-main-2.log:148`), so it is pre-existing and not
caused by an open lane. The two existing epoch-flip hand-authored cases pass; neither draws a watch
before the flip.

## Root cause
Not diagnosed. Hypothesis: the reboot of a rejected epoch flip drops the SUT's registered watches
on Turso, while the reference model (`inv-active-watches-match-ref`) keeps them across the reboot.
Either the SUT must re-register its watches after the reboot, or the model must drop them like
other in-memory state (`reboot_drops_in_memory`). Which side is right is open.

## Missing piece
The unpinned keystone smoke draws one case; it rarely draws Turso AND a SetupWatch before an
EpochFlipRejected. The generator can reach the sequence, so this is a draw-frequency (COVERAGE)
gap, not a missing transition or invariant. No hand-authored case combined a watch with the flip.

## Remedy
Open, no fix in this entry. Registered as known-red `epoch-flip-drops-active-watch` in
`docs/Testing/KeystoneKnownReds.md` (unowned; pending Martin's ratification) so classify scripts
call it known. Deterministic lock:
`crates/holon-integration-tests/hand-authored-regressions/known-red-epoch-flip-drops-active-watch.jsonl`
(replay with `HOLON_HAND_AUTHORED_SIDECAR=<file> HOLON_HAND_AUTHORED_SKIP= just hand-authored`).
