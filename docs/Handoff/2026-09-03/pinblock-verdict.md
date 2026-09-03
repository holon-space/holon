# Verdict: PRE-EXISTING (not caused by any lowcode-inc1 chain commit)

## Classification
**PRE-EXISTING** — reproduces on `main` (89e2efeaa1ff), the chain base, 3/3.
NOT `FLAKY`: 15/15 red, byte-identical MISS REASON at every revision.

## Deterministic replay built (this is the reusable artifact)
`just keystone-smoke` has no seed replay (proptest persists an RNG seed only).
The repo's deterministic path is `tests/hand_authored_regressions.rs` +
`HOLON_HAND_AUTHORED_SIDECAR` (out-of-tree A/B sidecar) + `HOLON_HAND_AUTHORED_CASE`.
Fixture authored verbatim from the log's `minimal failing input`
(w-lowcode-inc1-smoke.55633.log:13205-17500), wiring pinned to the failing draw
`storage={Turso} sync={} actors={}` (log:13088):
`/private/tmp/.../scratchpad/pinblock-probe.jsonl` (case `pinblock-bulk-1-2-probe`).
Transitions:
1. BulkExternalAdd doc=block:368857d2-... blocks bulk-1-0/1/2 ("a","a","A")
2. InstantiateTemplate parent=block:bulk-1-2 date="jndaxu" mood="qqnw"
3. NavigateBack region=Main
4. SplitBlock block:10f4e8ae-5f5e-58a2-8b9a-bbe50ce6607f pos=0
5. PinBlock region=RightSidebar block=block:bulk-1-2
The InstantiateTemplate-minted ids reproduce (transition 4 resolves), so the case
is fully replayable — it can be promoted into
`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl` as-is.

## Rate table (3 runs per revision, ~10s each)
| rev | bookmark | result |
|---|---|---|
| 6b6e47c3595c | sw/lowcode-inc1 (tip) | RED 3/3 |
| e33bfa871948 | sw/ingest-contract | RED 3/3 |
| d596702637dd | sw/pair-inc1 | RED 3/3 |
| ff7448cc5bf0 | sw/org-writeback-reds | RED 3/3 |
| 89e2efeaa1ff | main | RED 3/3 |

Identical panic at every rev (`components.rs:3297`, `:3277` on main — same
assertion, line shifted):
`MISS REASON: block:bulk-1-2 renders NO node in region main … renders 4 distinct
entities: [block:368857d2-…, block:__virtual:368857d2-…, block:default-main-panel,
block:journals]`

## Causal path
* `BulkExternalAdd` inserts bulk-1-0/1/2 as children of the auto-created journals
  day page `block:368857d2-…` (`PageId::for_path("Journals/2026-01-16")`).
* `NavigateBack(Main)` sets `focus_roots(main) = block:journals` — confirmed in
  the panic text.
* Under that focus root the main panel renders the day page **as a lazy embedded
  page shell**: the day page node plus its virtual slot
  `block:__virtual:368857d2-…`, and **none** of its children. So `bulk-1-2` has no
  bullet, the shift-click binds no `focus_pin` intent and degrades to bare focus.
* NOT the pair-inc1 projection withhold, NOT the ingest metadata reconcile, NOT
  the write-back membership settle fix. Evidence: (a) red on `main`, which
  predates all three; (b) the `[FileSyncController] write-back SKIPPED …
  membership` WARN at log:13115 is not causal — the same WARN fires benignly in
  every green hand-authored case at the tip (e.g. `block:cb60abcd-…` in
  `echo_loop_block_to_page_child_render_leak_parked`, which passes).
* Reduced probe (transitions 1+5 only, `pinblock-reduced.jsonl`) is red for a
  DIFFERENT and vacuous reason — boot focus is `block:structural-page`, renders
  `[__virtual:structural-page, c1, c2, default-main-panel, parent,
  structural-page]`. This pins `NavigateBack → journals` as the step that makes
  the case non-vacuous, and confirms the miss is journals-feed lazy embedding,
  not a lost write.

## Open (for the orchestrator, not the verifier)
Whether the journals feed SHOULD render day-page children inline (prod render
drop, sibling of the registered `main-panel-drops-refocused-split-block` known
red), or whether `PinBlock`'s generator precondition must exclude blocks behind a
lazy virtual slot (keystone precondition gap). Same family as
`inv-embedded-page-collapsed-lazy` / `inv-viewmodel-tree-virtual-slots`.
The gate at 6b6e47c3 should NOT be blocked on this chain.

## Provenance
Workspace `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/_sw_integ`
(pwd asserted in every run). @ restored to a new empty rev on 6b6e47c3595c.
Lane logs: `<ws>/lane-logs/pinblock-{tip3,e33bfa87,d5967026,ff7448cc,main89e2,reduced}-*.log`
