# SplitBlock seed=42 — follow-up after re-running with SQL probe

Worktree: `.claude/worktrees/splitblock-seed42-debug`
Repro: `PROPTEST_SEED=42 cargo test -p holon-gpui --test gpui_ui_pbt --features pbt`
Log: `/tmp/gpui_pbt_seed42_probe.log`

## TL;DR — open task #18 no longer reproduces

The previous handoff (`devlog/2026-05-16-splitblock-seed42-handoff.md`) listed task
#18 as the blocker: "WAITING-tagged block c2f12z-s doesn't render in Main after
NavigateFocus." That failure is **gone** on this run, without changing any
production code. Seed=42 now progresses 6 steps further and fails at step 10
SplitBlock with a different bug class.

The added `probe_block_sql_state` diagnostic (sut.rs, called on
`wait_for_entity_bounds` timeout in SplitBlock) was never triggered — the
timeout never fired.

`/tmp/gpui_pbt_seed42_probe.log`:

- L1102: post-NavigateFocus tree contains
  `rendered-text-block:c2f12z-s-content … entity_id=Some("block:c2f12z-s")`.
- L1197: step 4 SplitBlock targets c2f12z-s, click resolves topmost to the
  c2f12z-s rendered_text. Step 4 ✓.
- Steps 4–10 SplitBlock all succeed.

The task_state-filter hypothesis the prior handoff suggested was wrong; the
block_profile `default` variant already renders task blocks via
`column(row(selectable, draggable, state_toggle, spacer, rendered_text), drop_zone)`
with no exclusion for `is_task`.

Why this run differs from `gpui_pbt9.log` isn't fully proven, but the
OUTBOUND aggregate_ids order changes between the two runs:
- pbt9: `[ref-doc-0, c2f12z-s, -q--2b-9..., nvhz...]`
- now:  `[ref-doc-0, -q--2b-9..., nvhz..., c2f12z-s]`

Likeliest explanation: small scheduling differences (e.g. OrgMode
propagate-wait timing — handoff fix #3) shifted the create order so the
WAITING block lands in the same CDC batch as its siblings, and the matview
projection no longer drops it. Either way, the failure mode is gone.

## New failure: step 10 SplitBlock content divergence

Step 10 splits `block:76db4c04-…-a0509ec64a47`. After the chord runs, prod
and ref disagree about where the cursor was when split fired
(`assertions.rs:60`):

| Block id      | Ref content        | Prod content   |
|---------------|--------------------|----------------|
| 76db4c04…     | `"d"`              | `"d83xI"`      |
| 8c2a6bef…     | `"83xI c8NQQ"`     | `"c8NQQ"`      |

Concatenation is identical (`"d83xI c8NQQ"`); prod split four chars later
than ref. Same class as the long-standing "split-cursor predicted from
last-committed text" bugs — ref likely tracked an `active_editor` cursor
that diverged from prod's live `InputState::cursor()` by 4 chars after the
last preceding edit.

History under `block:ref-doc-0` immediately before step 10 (from L2750
context): nine prior SplitBlock results live there, with the click
targeting 76db4c04 at coords `(792, 179)`. The 4-char drift suggests an
earlier `TypeChars`/SplitBlock pair where the ref model committed and
trimmed but the prod editor kept four typed chars staged.

## Next steps for whoever picks this up

1. **Don't chase #18 further** — drop it from the open list. The
   children-settled gate + org propagate-wait fixes from the prior session
   are load-bearing for the new baseline. Keep them.
2. **Step 10 cursor divergence** — read L1900–2760 of
   `/tmp/gpui_pbt_seed42_probe.log` to find the TypeChars / SplitBlock
   sequence on 76db4c04 that opened the 4-char gap. The fix is likely on
   the ref side: ensure `commit_active_editor_if_changed` runs before
   SplitBlock cursor read, matching what prod's Enter handler sees
   (`crates/holon-integration-tests/src/pbt/reference_state.rs`).
3. **SQL probe is generic** — `E2ESut::probe_block_sql_state(&self, &str)`
   queries `block_raw`, `block` matview, sibling rows under same parent,
   and `focus_roots`, formatted as a multi-line string for embedding in
   panic messages. Reuse it from other transitions' bounds-timeout panics
   (Indent/Outdent/ClickBlock) when the next "block missing from render"
   class surfaces.

## Files touched in this session

```
crates/holon-integration-tests/src/pbt/sut.rs   # probe_block_sql_state helper +
                                                # SplitBlock bounds-timeout enrichment
tools/src/turso_sql_replay.rs                   # added missing `use std::sync::Arc;`
                                                # (pre-existing compile error blocking
                                                # WorktreeCreate hook)
```

## Update: deeper run with bumped timeout

Re-running with extra `[SplitBlock-presplit]` instrumentation revealed the
"#18 c2f12z-s render" failure mode is a **timing flake**, not a fundamental
filter bug. The `wait_for_children_settled` gate was set to 2s; on a slow CDC
batch one of the three ref-doc-0 children (rotating between c2f12z-s and
nvhz depending on Loro merge order) takes longer than that to render its
`rendered_text` widget. Bumping the gate to 5s eliminates the failure and
pushes seed=42 from step 4 to **step 15** before the next divergence.

Change: `crates/holon-integration-tests/src/pbt/sut.rs` —
`wait_for_children_settled(..., Duration::from_secs(2))` →
`Duration::from_secs(5)` (at the SplitBlock call site).

### Step 15 — new failure: block_raw count mismatch

`/tmp/gpui_pbt_seed42_presplit2.log:3743-3748`:

```
[SplitBlock count-mismatch diag] expected=29 db_rows=28 unique_ids=28
  missing_from_block_raw=["block:ref-doc-0", "block::split-13",
                          "block:journals", "block:ref-doc-1",
                          "block:4039728b-4e9a-4978-98b2-30bb07811918",
                          "sentinel:no_parent"]
panicked at sut.rs:2378: assertion left==right (28 vs 29)
```

The "ref-doc-*"/journals/sentinel ids are expected absences from
`block_raw`. The real miss is `block:4039728b-…-30bb07811918` — a block
that ref-state believes exists but doesn't appear in `block_raw` when
SplitBlock checks `expected_block_ids` against the materialized count.

Most likely path: 4039728b was created by an earlier SplitBlock or
WriteOrgFile but the Loro→SQL propagation hasn't landed yet by the time
step 15's `wait_for_blocks_synced` polled. Same timing class as the
children-settled flake — `wait_for_blocks_synced` has a 5s timeout that
may not be enough on a heavy CDC batch.

## Open tasks (refreshed)

| ID  | Status | Subject |
|-----|--------|---------|
| #14 | done   | Verify hit-test tie-break |
| #15 | done   | Children-settled predicate |
| #17 | done   | OrgMode parser places block before create lands in Loro |
| #18 | done   | c2f12z-s render — was a 2s timeout flake; bumped to 5s |
| #16 | open   | Wrapper editable_text swallows synthetic click |
| #20 | open   | Step 15 SplitBlock — block:4039728b missing from `block_raw` (CDC lag past 5s wait_for_blocks_synced timeout — bump or change to ref-state-equality gate) |
