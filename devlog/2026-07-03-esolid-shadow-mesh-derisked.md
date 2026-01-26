# E-solid shadow-mesh oracle: de-risk COMPLETE — implementation handed off

**Worktree:** `composed-pbt-boot-parallelism`. Follows
`devlog/2026-07-03-peer-sibling-order-fixed-lever1-relanded.md`.
**Handoff (implementation plan, hazards, deletion list):**
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon--claude-worktrees-composed-pbt-boot-parallelism/bb22cd1b-b3ea-4562-bc59-183a36bd36f8/scratchpad/handoff-esolid-shadow-mesh-oracle.md`

## Why

The lever-1 fix used check-time **SUT-adoption** (verified, but still SUT→Ref data flow) for two
CRDT-arbitrary dimensions: tied-sibling order and concurrent-text interleaving. The user flagged
adoption as a documented anti-pattern (previously abused, hid real bugs) and asked whether the
Ref could run Loro itself. Answer: yes — **if** the shadow universe's op-id tie-breaks
`(lamport, peer)` line up, which requires only **scalar Lamport-height clock sync** at
fork/sync/primary-edit boundaries (never content). Loro's tie comparator is `pub(crate)` (not
importable), but `LoroDoc::get_change().lamport` is public — so the model *runs* loro instead of
reimplementing it, and auto-tracks loro upgrades.

## De-risk results (all in-tree, all green: lib 160/160, loro 132/132)

- `holon-loro/src/multi_peer.rs::clock_parity_spike` — 8 tests: peer-id tie-break at equal
  lamport, lamport dominance, staggered fork heights, text interleaving, causal sanity, kitchen
  sink, **negative control** (unpadded shadow DIVERGES — padding is load-bearing, tests have
  teeth), **base-op-shape independence** (only base strings + peers' own op ids matter — the
  production boot history's shape is irrelevant).
- Walking skeletons vs the REAL production boot (structural_pbt teeth):
  `shadow_mesh_predicts_sut_peer_merge_exactly` (peer-only) and
  `shadow_mesh_predicts_concurrent_primary_peer_merge` (real editor TypeChars concurrent with a
  peer edit; **lamport-exact mirroring**: pad to the post-previous-transition height, mirror,
  then let the SUT apply). Both predict order + interleaving EXACTLY.
- New plumbing: `loro_backend::doc_lamport_height` (prod-visible),
  `multi_peer::{lamport_height, pad_to_height}` (pub), cap
  `SutLoroLog::loro_lamport_height()` across all 5 impl/forward sites.
- Existing enabler discovered: primary peer id is already pinned (`HOLON_LORO_PEER_ID=1`,
  `ui_harness.rs`) — the shadow needs no peer-id read.

## Not yet de-risked

`ShadowDoc: Clone` as `fork() + set_peer_id(original)` (proptest deep-clone requirement) — the
handoff says to spike this FIRST in the implementation session.

## Rule for the implementation

Never half-swap the oracle: the adoption fns stay until the shadow mesh fully replaces them in
one session; the tree must stay green at every stop point.
