# Verify: quick-open-contrast (D96.a) — fresh-context adversarial

Workspace `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/quick-open-contrast`, `@-` = `830d794f878f` (asserted).
Tree assert passed: `selected_muted_fg` x2 in `frontends/gpui/src/search_ui.rs`; `painted_fg` in
`crates/holon-frontend/src/geometry.rs`.

## Overall: CONFIRMED (C1–C4, C6, C7). C5 NOT REPRODUCED — blocked, not failed.

## C1 — muted_on_selection >= 4.5:1 on every builtin theme — CONFIRMED (report has 2 cosmetic errors)
Read `crates/holon-frontend/src/theme.rs:41-92`. Reimplemented `muted_on_selection` + WCAG
`contrast_ratio` independently in `<scratch>/verify-contrast/wcag.py`, fed the 18 `primary`
values pulled from `assets/themes/*.yaml`:
- Holon Light `#2A7D7D` -> `#FFFFFF` **4.852** (lane: 4.85). Holon Dark `#5DBDBD` -> `#214242` **4.930** (lane: 4.93). Both match.
- All 18 clear 4.5. Range **4.716 – 5.172**, NOT the "4.72–4.95" the report states — the
  upper bound is wrong (GitHub Dark 5.17, Dark 5.13, Catppuccin Mocha 5.04). Cosmetic.
- "11 builtin themes" is 11 yaml FILES = **18 registry keys** (verified no key collisions).
  The test iterates all 18, but its own guard is only `checked >= 2` — it would still pass
  if the registry silently loaded 2 themes.
Stronger than claimed: brute-forced 85^3 fills — worst achievable ratio is **4.5826**, so the
floor holds for arbitrary USER themes too, not only builtins.
Production path checked: `apply_holon_theme` sets `accent = rgba8_to_hsla(c.primary)`
(`frontends/gpui/src/lib.rs:3842`), so the unit test's `colors.primary` IS the painted fill.
Hsla round-trip loss is <=1/255 per channel => <0.01 ratio, vs a 0.216 worst-case margin. Safe.

## C2 — red for the right reason, then green — CONFIRMED
`lane-logs/red-110501.log:207-208`: panic at
`quick_open_selected_row_contrast_windowed.rs:331` — "paints 1.11:1 against the selection fill
(text Some([107,107,101,255]) on [42,125,125,255])". A ratio assertion, not a missing symbol
(the binary linked and ran).
Reproduced green MYSELF (`<scratch>/verify-contrast/v-windowed.log`):
`PASS [8.414s] (1/1) holon-gpui::quick_open_selected_row_contrast_windowed
selected_hit_subtitle_clears_the_body_text_contrast_floor` / `Summary: 1 test run: 1 passed, 0 skipped`.

## C3 — cascade, no parallel registry — CONFIRMED
`PAINT_COLORS` is declared inside the SAME `thread_local!` block as `RENDER_PATH`
(`frontends/gpui/src/geometry.rs:271-277`); colours ride the existing `BoundsRegistry` via two
new `ElementInfo` fields. Grepped: no second registry type.
Cascade merge is `self.colors.fg.or(inherited.fg)` / `.bg.or(inherited.bg)`
(`frontends/gpui/src/geometry.rs:545-549`), pushed as the MERGED pair (`:583`) and popped after
the child prepaint (`:585`) — so a child with no explicit colour inherits the nearest painted
ancestor. This is the load-bearing path: the subtitle declares
`.with_painted_colors(Some(subtitle_fg), None)` (`search_ui.rs:425`) while the row declares
`(Some(row_fg), Some(row_bg))` (`search_ui.rs:438`).
Directly pinned, not just reasoned: the windowed test asserts
`subtitle.painted_bg == Some(row.painted_bg)`
(`frontends/gpui/tests/quick_open_selected_row_contrast_windowed.rs:313-317`) AND guards against
measuring the wrong row via `assert_ne!(row1.painted_bg, Some(fill))` (`:325-329`).

## C4 — accent pair untouched — CONFIRMED
`jj diff --git frontends/gpui/src/lib.rs`: the ONLY addition at the SearchTheme literal is
`selected_muted_fg`; `selected_bg: theme.accent` / `selected_fg: theme.accent_foreground` appear
as unchanged context. No diff to `assets/themes/*.yaml` or to `apply_holon_theme`
(lib.rs:3842-3843 untouched). Whole-diff stat = 11 files, 596+/32-, none of them a theme asset.

## C5 — gates — NOT REPRODUCED (blocked by machine-wide build famine)
`cargo nextest run -p holon-frontend` was launched under the wrapper and sat QUEUED for ~50 min:
its tee log stayed 0 bytes (= waiting on the slot, per the known 0-byte-means-running hazard).
Cause is genuine contention, not a wedge: the semaphore dir held **6 LIVE holders for 4 slots**
(all PIDs `kill -0`-alive; no stale slot files). Machine showed 51-54 cargo / 21 rustc.
`just hand-authored` was therefore not attempted. C5 is UNVERIFIED, in the "could not run"
sense — I have no evidence against the lane's 591/591 and 9/9.

## C6 — layout_smoke double badge: PRE-EXISTING, not caused by this lane — CONFIRMED (static)
The empirical base run could not be done (same famine; the base tree WAS extracted non-empty —
4447 files at `<scratch>/verify-contrast/base830d` — and its target CoW-seeded, 256G). Settled
statically instead, and the static evidence is decisive:
- `diff -rq base830d/frontends/gpui/src/render <workspace>/frontends/gpui/src/render` =>
  **RENDER_TREE_IDENTICAL_TO_BASE**. The whole builder/badge path is byte-identical.
- The `frontends/gpui/src/geometry.rs` diff is **purely additive — it contains not one `-`
  line**. Nothing changes `registry.record` call sites, `next_seq()`, or el_id generation, so
  the tracker cannot add a second badge record.
- The failing test renders `column(vec![text,text,badge,icon])` (`layout_smoke.rs:179`) and never
  touches search_ui or colours. This lane's only edit to that file is the two struct-literal
  fields in a helper (`layout_smoke.rs:245-249`).
So the lane's attribution (`badge.rs:23` self-tracks + `builder_registry!`'s `tag_node` at
`builders/mod.rs:35` tracks again) stands, and "nothing in this diff touches builders" is TRUE.
NOT a refutation. Residual: caused-vs-pre-existing is proven by construction, not by a base run.

## C7 — bugfunnel entry — CONFIRMED, but the claim's wording is wrong
It did NOT flip to FIXED. It flipped `OPEN` -> **`PARTIAL`**, which is a valid status
(`scripts/bugfunnel.py:29`, 20 other entries use it) and is the MORE accurate choice: the remedy
section names both covering tests and keeps a "STILL OPEN: the overlay's last content row is cut
mid-row" paragraph. `bugfunnel.py counts` runs clean (652 escapes, PARTIAL: 20).

## Defects found (evidence only — not fixed)
**D1. The fix does not cover the HOVER state — the original bug is still reachable.**
`frontends/gpui/src/search_ui.rs:398` `.hover(|s| s.bg(theme.selected_bg))` paints an
UNSELECTED row with the selection fill on mouse-over, while that row's subtitle keeps
`theme.muted_fg` (`:380-384`, the `else` arm). So hovering any non-selected quick-open hit
reproduces exactly the dogfooded teal-on-teal 1.11:1 subtitle. The line is pre-existing
(present at `base830d/.../search_ui.rs:371`, only re-indented by this lane), so this is an
incomplete fix rather than a regression. Neither new test can see it: the tracked record stores
the STATIC `row_bg`, so `painted_bg` never reflects hover.

**D2. The new colours are invisible to the MCP driver, so `dogfood-explorer` still cannot see
contrast.** `frontends/mcp/src/describe_ui_geometry.rs` serializes `opacity` (`:191-192`) but has
no `painted_fg`/`painted_bg` branch. The lane's only MCP-side edit is the two struct fields in a
TEST literal (`frontends/mcp/tests/describe_ui_geometry.rs`). The entry's PERCEPTION gap is
therefore closed for this one element by a windowed test, not for the surface generally.

## Gaps
- C5 unverified (build famine). Re-run `-p holon-frontend` and `just hand-authored` before landing.
- The end-to-end (hsla round-trip) contrast is pinned for the DEFAULT boot theme only; the other
  17 themes are covered by pure math, never through the real render path.
- `selection_subtitle_clears_the_body_text_floor_in_every_theme` guards `checked >= 2`, not 18 —
  a registry-load regression that dropped 16 themes would keep this test green.
- `pop_colors`/`pop_parent` are not unwind-safe; a panic inside a child prepaint leaks the stack.
  Same shape as the pre-existing `RENDER_PATH`, so not a regression.
