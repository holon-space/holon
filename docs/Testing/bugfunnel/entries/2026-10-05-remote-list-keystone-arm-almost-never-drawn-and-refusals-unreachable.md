---
id: 2026-10-05-remote-list-keystone-arm-almost-never-drawn-and-refusals-unreachable
date: 2026-10-05
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The keystone's RemoteListSync transition was drawn about once in seventy steps
  even in forced-full runs, and its fixture could not produce a duplicate entry,
  so the refusal path of a list sync was never exercised by the keystone.
---

## Bug
Found by the verifier of the shopping free-text-amount lane (round 2, defect D3).
`inv-remote-list-mirror-matches-ref` reported full engagement in forced-full runs,
but the failure dumps showed an empty peer list and zero `RemoteListSync` steps. The
invariant compared an empty mirror against an empty model. The refusal path
(`ListSnapshot::refusals` → `SyncOutcome.refused` → the provider's `warn!` and
`{"refused": [...]}` response) was pinned only by the holon-app mock test.

## Root cause
Two causes, both measured:
- Weight. `RemoteListSync` drew with weight 8. The other keystone transitions weigh
  about 1300 together (`MoveBlockBetweenFiles` alone is 250 and took 35 of 182 draws).
  `HOLON_PBT_TELEMETRY=1 HOLON_PBT_FORCE_FULL=1`: 1 draw in 71 transitions over 5 cases
  (lane-logs/r5-d3-telemetry-2.log). The only precondition is `app_started`, so it
  was not a precondition that blocked it.
- Fixture. The keystone fixture declared `refusal_row_type: None`, and
  `FixtureListPeer` stored one row per key, so no list could hold a duplicate.
  `just keystone-smoke` deselects the arm entirely (its drawn wiring has no
  remote-list cap); this is the same reachability class as
  2026-09-14-remote-list-sync-keystone-unreachable.

## Missing piece
No draw-count check for a newly added transition, and no fixture shape for the
refused-entry branch of the connection contract.

## Remedy
`crates/holon-integration-tests/src/pbt/transitions/remote_list_sync.rs`: weight 130
while the peer list is empty, 65 after → 7 draws in 119 transitions over 5 forced-full
cases (lane-logs/r5-d3-telemetry-6.log). The fixture declares `content_refusal`;
`ListMutation::AddDuplicate` adds a later entry under a held key, and the peer keeps the
first and emits one refusal per duplicated key. The model keeps the first entry's cells
and counts one refusal per duplicated key; the transition asserts both rounds report
that count. The hand-authored case
`remote-list-local-amount-reaches-the-peer-and-a-duplicate-keeps-the-first` replays it
deterministically: red when the provider withholds the refusals
(lane-logs/r5-red-refusal-keystone.RED.log) and when the peer keeps the last entry
(lane-logs/r5-red-keepfirst-keystone.RED.log).
