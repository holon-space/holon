---
id: 2026-10-05-epoch-flip-reboot-drops-an-active-watch-on-turso
date: 2026-10-05
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  With the keystone wiring pinned to Turso, SetupWatch followed by
  EpochFlipRejected failed inv-active-watches-match-ref, because the reference
  model kept the query watch across the reboot, which no production boot does.
---

## Bug
Found by a lane's keystone gate under `HOLON_PBT_PIN_WIRING="Turso;;"` (json-prop-boot lane,
`lane-logs/sas-r2-gate-ks-turso.log:30065`). The shrunk input is
`[SetupWatch(query-a), EpochFlipRejected]` (query-a: Blocks, columns id/content/content_type/
source_language/source_name/parent_id, AllBlocks, holon_prql). Verdict:
`[inv-active-watches-match-ref] watch sets diverged  missing on SUT: ["query-a"]  spurious on SUT: []`.
It reproduced 2 of 2 as a hand-authored case on main b2a19917 and again on e0313eaf
(epoch-flip lane, `lane-logs/repro-2.log:420`).

## Root cause
The reference model was wrong; the SUT behaves like production. A query watch lives in the boot's
`ReactiveEngine` registry (`HeadlessFrontendComponent::register_watch_compiled`), and nothing
persists it. The reboot (`HeadlessFrontendComponent::reboot_through`, shared by `Reboot` and
`EpochFlipRejected`) drops it with the dead session. The reference's
`RefReboot::reboot_drops_in_memory_state` (`crates/holon-integration-tests/src/pbt/ref_caps/boot.rs`)
did not clear `mcp.active_watches`, so it expected the watch after the reboot.

Measurements that tie the cause to the effect (epoch-flip lane):
- The same sequence with `Reboot` in place of `EpochFlipRejected` reds with the identical
  signature (`lane-logs/probe-reboot-red.log:125`): the cause is the shared reboot, not the flip.
- With only the reference change (clear `mcp.active_watches` on reboot), both sequences pass
  (`lane-logs/probe-green.log:124,210`).

Only the epoch flip exposed it: `Reboot` is off in random draws (`HOLON_PBT_REBOOT`), while
`EpochFlipRejected` is drawn on every Turso wiring.

## Missing piece
None in the product. The test asserted a promise production never made (a query watch that
survives a restart); a client registers its watch again after a boot.

## Remedy
`reboot_drops_in_memory_state` clears `mcp.active_watches` (FIXED in the epoch-flip lane commit,
D96.a, on top of e0313eaf). Locked by two green hand-authored cases in `keystone.jsonl`:
`a-query-watch-ends-with-the-epoch-flip-reboot` and `a-query-watch-ends-with-the-reboot`. The
known-red row `epoch-flip-drops-active-watch` and its sidecar are removed.
