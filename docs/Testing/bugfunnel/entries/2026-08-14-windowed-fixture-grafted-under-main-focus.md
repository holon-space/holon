---
id: 2026-08-14-windowed-fixture-grafted-under-main-focus
date: 2026-08-14
gap: ENVIRONMENT
secondary: null
status: UNCLASSIFIED
summary: >-
  Every windowed fixture grafted under the Main focus root was invisible to
  the window, across six test binaries, because a fresh vault focuses Main on
  `block:journals` — a query page whose feed selects only `Page`-tagged
  children.
source_line: 701
---

## Bug

(task-#28 windowed-slice-revive lane; found by agent exploration of the
binaries task #24 had just made compilable, no gate reported it) **Every
windowed fixture grafted under the Main focus root was invisible to the
window, across six test binaries, because a fresh vault focuses Main on
`block:journals` — a query page whose feed selects only `Page`-tagged
children.** `holon-app/src/seed.rs:176-196` lands first launch on the
Journals overview; `assets/default/Journals.org` gives that block a
`holon_sql` source (`SELECT * FROM journal_feed …`) so the panel paints the
FEED; `holon/tests/journal_feed_matview.rs:10` pins the feed to
`Page`-tagged children only. The graft helpers resolved their parent through
`main_focus_root()` = `focus_roots WHERE region='main'` = `block:journals`,
and graft PLAIN rows. Measured: rows reach `block_raw` and stop (`vm_rows=0
seeded_rows=3 elements=58`), with the identical 58-element ceiling in the
independent `cmd_enter_chord_dispatch` binary.

## Root cause

task-#28 windowed-slice-revive lane, found by agent exploration of the
windowed binaries that task #24 had just made compilable again — no gate
reported it, because none of these targets is in one: **every windowed
fixture grafted under the Main focus root has been invisible to the window,
in SIX test binaries at once, because a fresh vault focuses Main on
`block:journals` — a QUERY page whose feed selects only `Page`-tagged
children.** `crates/holon-app/src/seed.rs:176-196` lands first-launch users
on the Journals overview via `navigation::focus`;
`assets/default/Journals.org` gives `block:journals` a `holon_sql` source
child (`SELECT * FROM journal_feed ORDER BY content DESC`) plus a `render`
child, so the main panel paints the FEED, not the block's plain children;
and `crates/holon/tests/journal_feed_matview.rs:10` pins that `journal_feed`
"contains exactly the `Page`-tagged children of `block:journals`". Every
graft helper in
`crates/holon-integration-tests/src/pbt/window_slice/seed.rs` resolved its
parent through `main_focus_root()` = `focus_roots WHERE region='main'` =
`block:journals`, and grafts PLAIN rows — structurally excluded from the
only query the panel renders. Measured: the rows reach `block_raw` and stop
there (`lane-logs/wslice-reactor-fix.log:962`, `[nested-live-block] after
model settle: vm_rows=0 seeded_rows=3 elements=58`), and an INDEPENDENT
binary shows the identical ceiling (`lane-logs/control-cmd-enter.log`,
`[cmd-enter-boot] chord target not painted yet; 58 elements`) — the 58
elements being the 3-column shell plus `expand_toggle::journals` /
`vms_button::block:journals#qsrc::{source,result}`, i.e. the journals page
rendered AS a query block. ENVIRONMENT, not a windowed data-path defect:
write path, matview chain and paint path all behave correctly; what broke is
the fixtures' assumption that the Main focus root renders its plain
descendants as an outline — an assumption a PRODUCT DEFAULT (first launch
landing on the Journals overview) silently invalidated.
`gpui_journals_viewport.rs` is the control that stayed honest: it grafts
`Page`-tagged day pages, which the feed does select, and explicitly
navigates onto `block:journals`. FIXED at the shared helper:
`main_focus_root()` is replaced by `focused_graft_root()`, which creates the
plain page `block:wslice-graft-page` and runs `navigation::focus` for
`Region::Main` onto it before returning its URI, so all six helpers — and
with them `gpui_window_slice`, `cmd_enter_chord_dispatch`,
`window_chord_reentrant_dispatch`, `undo_survives_blur_windowed`,
`task_keyword_blur_windowed`, `structural_chord_stale_flush_windowed`,
`live_promotion_windowed` — hang their fixtures where the panel actually
renders them. MEASURED EFFECT, and it is a repair of the DATA half only:
`focus_roots(region=main)` moves from `block:journals` to
`block:wslice-graft-page` (`lane-logs/rv-diag.log`), and the band's
ViewModel goes from `vm_rows=0` after 538 settle rounds to `vm_rows=18` at
round 1 (`lane-logs/rv-wslice-after.log`), so the fixture now reaches the
engine. The PAINT half does not follow: the focused page's `live_block`
mounts with `has_content=false` and paints NONE of its subtree — not the
band's 18 rows, not the plain `BANDPRE-`/`BANDSIB-` rows, and not the single
plain row `cmd_enter_chord_dispatch` grafts (68 elements, the shell plus the
page's own row, in both binaries). All four `gpui_window_slice` tests and
the `cmd_enter` control therefore remain red ON THAT, which is exactly what
`nested_live_block_paints_the_rows_its_model_holds` exists to judge — see
the report `lane-logs/wslice-revive-report.md` for why the two candidate
causes (a freshly created top-level page whose org document never settles,
vs. a genuine `live_block` layout defect of the #60 class) were not
separated in this lane. NOT CLOSED at the structural level either, and it is
the same hole task #24 left open: nothing runs these binaries in a gate, so
the next product-default change can blind them again in silence.)

## Missing piece

The fixtures assumed the Main focus root renders its plain descendants as an
outline; a product default (first launch landing on the Journals overview)
silently invalidated that, and nothing runs these binaries in a gate, so no
signal existed. `gpui_journals_viewport.rs` stayed honest only because it
grafts `Page`-tagged day pages and navigates explicitly.

## Remedy

DATA HALF FIXED at the shared helper — `main_focus_root()` replaced by
`focused_graft_root()`, which creates the plain page
`block:wslice-graft-page` and runs `navigation::focus` for `Region::Main`
onto it. Measured: the main focus root moves off `block:journals`, and the
band's ViewModel goes from `vm_rows=0` after 538 rounds to `vm_rows=18` at
round 1. PAINT HALF STILL RED: the focused page's `live_block` paints none
of its subtree (68 elements in both `gpui_window_slice` and
`cmd_enter_chord_dispatch`), leaving all five tests red on the assertion
they exist for. Cause not separated in this lane — see
`lane-logs/wslice-revive-report.md`. NOT CLOSED structurally: no gate runs
these binaries (same open hole as the task-#24 row).
