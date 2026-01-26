# Keystone perf: the widget-snapshot resample loop was 83% of wall time — fixed (2.3×)

Fresh chrome-trace (post-convergence-settle, post-shadow-mesh) OVERTURNED the boot-focused
plan: boot is ~1 s/case (`di.*` total; the old multi-second waits are gone). The dominator
was **3,213 × ~124 ms main-thread sleeps (396 s of a 475 s run) between `interpret_pure`
calls** — `HeadlessFrontendComponent::widget_tree_snapshot`'s stability loop.

## Root cause (two stacked defects)

1. `view_model_to_snapshot` mapped BOTH `ViewKind::Empty` and `ViewKind::Loading` to
   `widget_name() = None` → kind `"unknown"`. The booted tree permanently contains one
   `Empty` slot (`…/tree/tree_item/<empty>`), so the snapshot loop's pending detector
   (`kind == "loading" || "unknown"`) NEVER saw a resolved tree…
2. …which forced `widget_tree_snapshot`'s cautious exit — 4 consecutive stable samples at
   120 ms — on EVERY check: ~480–600 ms of pure sleep per check, ~83% of keystone wall.

## Fixes (all landed, uncommitted, this worktree)

- `sut_capabilities.rs::view_model_to_snapshot`: name `Loading` → `"loading"` and `Empty`
  → `"empty"` explicitly (parse-don't-validate at the snapshot boundary — transient vs
  permanent placeholders are opposite things to a consumer).
  `viewmodel_root_matches_render_expr::is_not_ready_kind` gained `"empty"`.
- `components.rs::widget_tree_snapshot`: early exit on the FULLY-RESOLVED fixed point
  (pending == 0 + one confirming resample); a tree still holding placeholders keeps the
  old cautious 4×120 ms exit; 5 s deadline unchanged.
  ⚠ Tried and REVERTED: a 20 ms resample cadence — each resample drives `ensure_watching`
  (watch views + SQL), and the tighter poll churned CDC enough to inflate the NEXT
  transition's quiet-floor settle (p50 5 ms → 70 ms) and time the keystone out at 1200 s.
  The early exit is the win; keep the 120 ms cadence.
- Boot settle: `new_with_loro`'s flat 300 ms sleep → `converge_signals` (new shared
  `pbt/convergence.rs`, extracted from `wide_e2e::converge_projections which now
  delegates), capped at the old budget; polls for the lazily-resolved `OrgSyncIdleSignal`
  within the budget (a config without org sync pays the full budget = old behavior).
- NEW guard `booted_widget_tree_has_no_pending_placeholders` (structural_pbt teeth): a
  booted quiescent tree must hold zero loading/unknown nodes — a new unnamed `ViewKind`
  would silently re-impose the 4×120 ms tax on every check; now it fails loudly instead.

## Numbers (PROPTEST_CASES=16 + 10 cc seeds = 26 cases, uncontended, same machine)

| run | wall | notes |
|---|---|---|
| before (same code minus these fixes) | 690 s | keystone4 |
| after | **295 s** | keystone5 — **2.3×** |
| lib suite | 91 s → **51 s** | same fixes help every slice |

Per-transition settle healthy after revert of the 20 ms cadence: avg 54 ms, p50 60 ms.
A/B chrome-trace artifacts: scratchpad `boot-trace.json` (old) / `boot-trace2.json` (mid).

## What's next on this track (if more is wanted)

- Remaining floor: ~120 ms confirm-resample per check (~77 s/run) — could drop to a single
  sample once trust in the fixed-point proof is established, or hook the same quiescence
  signals instead of resampling.
- CPU now co-dominant: `interpret` 718 k calls/run — interpreter caching is the next lever.
- Boot options (DDL-scheduler unthrottle, arm parallelization) are now minor (~1 s/case).
