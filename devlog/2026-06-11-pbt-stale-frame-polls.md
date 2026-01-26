# Two PBT poll-loop bugs: post-scroll miss + frozen CachingProxy snapshot

Date: 2026-06-11 (afternoon session, worktree gpui-pbt-speed). Both are
follow-ups from the Phase A handoff (tasks #10/#11): the two remaining
no-loro red faces after "structural ops are commit points" landed.

## 1. Bounds-timeout "truly absent" face (task #11)

Signature: `wait_for_entity_bounds: timed out after 5s ... element truly
absent from rendered output` — yet the failure dump, taken milliseconds
later, lists the element WITH valid bounds and the right `entity_id`
(`a5_noloro2.log`, `probe11_run4.log` — identical pattern in both).

Root cause (harness): loop ordering in `wait_for_entity_bounds`
(`pbt/sut.rs`). Per iteration: found-check → scroll RPC (whose handler
pumps `window.refresh()`, so the revealed row commits DURING the RPC
await) → deadline-dump → `geometry.changed().await`. After a scroll the
loop never re-checked; it awaited the NEXT commit — by which time the
viewport can have snapped back and the row is evicted from the committed
pass. The reveal frame fell in a blind spot on every 300ms scroll cycle.
The timeout dump runs right after the final scroll, hence "present ms
later".

Fix: immediate re-check after the scroll RPC (logs
`[bounds-wait] ... revealed-by-scroll`). Probe added in
`scroll_entity_into_view` (gpui lib.rs): logs `[scroll-reveal]` with
`logical_scroll_top` before each reveal — a repeating `before` offset for
the same entity in the next red run will identify WHAT snaps the
viewport back (suspects: per-Replace splice churn in
`ReactiveShell::apply_diff` — `Replace` arrives on every MCP sync and
`cx.notify()`s, matching the ~69fps frame churn observed during the 5s
wait; or an autoscroll-to-focused-editor path).

Open: the snap-back source itself. The fix unblocks the wait; if
coordinates go stale before the click, the click path's own re-resolve +
re-click loops absorb it (and the probe will show it).

## 2. inv-window-focus-matches-engine-focus after NavigateHome (task #10)

Signature: `engine.focused_block() = None`, one window-focused editor in
the committed frame (the block a SplitBlock created 3 steps earlier),
all data layers green (`a5_noloro1.log`).

Root cause (harness, two layers):
- The invariant's 1s "poll until settled" loop calls
  `sut.rendered_elements()` — but invariants dispatch against the
  per-tick `CachingProxy`, which MEMOISES `rendered_elements`. Every
  retry re-read the same frozen snapshot; the invariant's explicitly
  allowed 1-2 frame window-focus lag becomes a guaranteed "settled"
  failure whenever the tick's first snapshot lands inside the lag
  window. The mechanism it polices (`spawn_focus_binding` blur-on-leave +
  `go_home → set_focus(None)` mirror) is present and was probably mid
  flight.
- Even with fresh reads, an occluded window (the zombie
  `reactive_vm_realwindow_test` processes held key focus all day —
  `[interaction-pump] CONTAMINATION` on every dispatch in these runs)
  commits no frames on its own, so the post-blur frame may never commit.

Fix: new `SutLayout::rendered_elements_fresh()` (default delegates;
documented contract: poll-style invariants MUST use it). CachingProxy
bypasses + refreshes its memo; `E2ESut` impl pumps one frame first via a
no-match scroll RPC (`block:__pbt-frame-pump__` — the ScrollEntityIntoView
arm calls `window.refresh()` unconditionally). The invariant body now
polls the fresh variant.

## 3. Wrong-block click (revealed by the #11 fix; evening batch)

After fixes 1+2, the no-loro batch produced a NEW dominant face 2/4:
`[SplitBlock] click_entity did not focus X before Enter (after re-click
attempts)` — every re-click hit the SAME coords and focused the SAME
wrong block (`a5_noloro1/4.log`, evening runs).

Root cause (one level deeper than #11): recorded bounds are CLIPPED to
the content mask (`visible_bounds` in the GPUI tracker). A row rendered
by `gpui::list` overdraw just outside the viewport registers a
DEGENERATE rect (e.g. `(436,50 764x0)`) at the clip edge. Two effects:
- `wait_for_entity_bounds` accepted any registry entry → returned
  instantly without scrolling the row into view;
- `element_center` resolved the degenerate rect's center — a point ON
  the clip edge, i.e. on top of whichever row is actually displayed
  there — so the click focused that other block, deterministically, on
  every retry. (This also retro-explains part of face #1: a clipped
  entry only "appears" once a reveal scroll un-clips it.)

Fix: `ElementInfo::has_visible_area` + `find_by_entity_id_visible`
(holon-frontend geometry); visible-area gate in both
`wait_for_entity_bounds` probes; `GpuiUserDriver::element_center` /
`element_center_in_region` skip degenerate rects (a clipped row now
resolves to NO center instead of a wrong point); SplitBlock's re-click
loop re-reveals via `wait_for_bounds` on resolution failure instead of
panicking.

Note: the zero-height rect also explains the "empty block" red herring —
the target was a split-created `""` block, but `rendered_text` renders a
placeholder for empty content; the 0-height came from clipping, not from
the empty string.

## Hazards hit

- jj reconcile trap twice: `yopyuzrx` absorbed into rewritten main
  mid-session (took the morning's sut.rs/lib.rs edits along — kept);
  foreign cosmetic fmt hunks leak into @ repeatedly (restored from @-).
- Main (`smqlpyom` @ 4a9ebb8c) currently does NOT compile (33 Arc<str>
  errors in `loro_sync_controller.rs`, concurrent session mid-flight) —
  validation of these fixes blocked until it does.
