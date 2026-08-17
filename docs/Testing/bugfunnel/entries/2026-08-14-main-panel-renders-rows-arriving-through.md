---
id: 2026-08-14-main-panel-renders-rows-arriving-through
date: 2026-08-14
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  The Main panel renders rows arriving through a page's QUERY SOURCE and
  renders NONE of a focus root's plain outline children
source_line: 700
---

## Bug

(task-#28 windowed-slice-revive lane; found by agent exploration once the
lane's own harness fix removed the mask hiding it) **The Main panel renders
rows arriving through a page's QUERY SOURCE and renders NONE of a focus
root's plain outline children**, measured across three focus roots —
`block:journals` (`Page`, query-rendered, seeded, 58 elements),
`block:wslice-graft-page` (`Page`, plain, fixture-created, 68),
`block:journals::auto-create` (non-`Page`, plain, boot-seeded, 59) — zero
plain children painted in any. The band's ViewModel holds all 18 rows while
`painted_band=0 pre=0 sib=0 head=0`. The only row that paints in the main
panel anywhere in this lane is the day page `2026-08-14`, via
`block:journals`' own `SELECT * FROM journal_feed`, so the panel is not
inert.

## Root cause

secondary COVERAGE, task-#28 windowed-slice-revive lane, found by agent
exploration after the lane's own harness fix removed the mask that had been
hiding it: **the Main panel renders rows that arrive through a page's QUERY
SOURCE and renders NONE of a focus root's plain outline children — measured
across three different focus roots, so a windowed fixture can sit in the
ViewModel and never reach the screen.** Once the graft-root drift (row
below) was fixed, the band's ViewModel went to `vm_rows=18` while
`painted_band=0 pre=0 sib=0 head=0` held — not one plain row, not the band's
rows, not the section headline. A discriminator run
(`lane-logs/rv-discriminator.log`, `cmd_enter_chord_dispatch`, 193.82s)
grafted a single plain row under a block SEEDED AT BOOT into an
already-written document instead of a page the fixture creates, and it did
not paint either: 1026 rounds of `chord target not painted yet; 59
elements`. Three roots now measured, zero plain children painted in any:
`block:journals` (`Page`, query-rendered, seeded — 58 elements),
`block:wslice-graft-page` (`Page`, plain, fixture-created post-boot — 68),
`block:journals::auto-create` (non-`Page`, plain, seeded — 59). The single
row that DOES paint in the main panel anywhere in this lane is the day page
`2026-08-14`, and it arrives through `block:journals`' own `SELECT * FROM
journal_feed` — so the panel is not inert and the defect is specific to
plain outline rendering. BOTH single-variable explanations are now dead by
direct measurement. "The fixture created the page after boot and its org
document never settled" is insufficient — a boot-seeded block in a settled
document behaves identically (`block:journals::auto-create`, 59 elements).
"The panel needs a `Page` focus root" is excluded too, and by evidence
already in hand: `block:wslice-graft-page` IS `Page`-tagged, since
`focused_graft_root` builds it via `create_page_block`, which sets `tags:
[PAGE_TAG]` (`window_slice/seed.rs:409-418`) — so a `Page`, plain,
query-source-free focus root painting zero plain children was already the
main measurement. Residual imprecision, disclosed rather than papered over:
the first discriminator's lookup (first text-typed child of
`block:journals`) returned `block:journals::auto-create`, the auto-create
RULE heading (`holon-frontend/src/lib.rs:85,139`), so that probe flipped two
variables at once; and the fourth cell (`Page` + pre-existing + plain + NO
query source) turns out to be STRUCTURALLY UNREACHABLE at graft time — the
corrected `block_tags`-join lookup returned nothing and failed loud in 2.46s
("the booted vault has no Page-tagged child of block:journals — the
daily_journal rule did not run"), because fixtures graft immediately after
`start_app` while the rule watcher fires later in boot. Reaching that cell
is a fixture redesign, not a probe. CLASSIFICATION DELIBERATELY HELD OPEN,
and the earlier draft of this row overstated it: the probes above vary only
ONE axis on which the harness differs from the living exemplar
(`gpui_journals_viewport.rs`), and there are THREE. Axis 1, which page the
graft hangs under, is the one tested. Axis 2: the exemplar opens the window
FIRST and navigates by a real sidebar CLICK, whereas these fixtures call
`navigation::focus` before the window exists. Axis 3: the exemplar calls
`env.wait_for_cdc_quiescent(500ms, 120s)` immediately after grafting
(`gpui_journals_viewport.rs:161-163`, the only call site in the tree) and
the fixtures did not — the observed `[GPUI] pre-warm timeout — window will
open with loading state` plus `has_content=false` fit an unsettled backend
at least as well as a layout defect. Axis 3 is now ELIMINATED BY
MEASUREMENT: adding that exact wait at the end of all six graft helpers
changes nothing — `lane-logs/rv-cdcwait.log`, 183.56s, 1258 rounds of `chord
target not painted yet; 68 elements`, same assertion, same count as without
it (wait reverted; it did not earn a 120s-per-fixture critical path). Axis 2
is now ELIMINATED TOO (`lane-logs/rv-axis2.log`): a probe navigating to the
SAME page both ways — pre-window `navigation::focus` vs a post-window
`SimUserDriver` sidebar CLICK — yields `focus_roots` and `navigation_cursor`
rows IDENTICAL field for field (`root_id=block:wslice-graft-page,
history_id=2, added_ts=2026-08-14 22:44:08`; cursor `history_id=2`), so the
pre-window op writes a well-formed, correctly cursor-paired row with nothing
malformed for the panel to fail to resolve; and the click SUCCEEDED (the
driver found the entity in bounds) while the focus root's plain child still
did not paint (64→68 elements, `chord_target_painted=0` both sides). WITH
ALL THREE HARNESS AXES ELIMINATED the product-defect reading is the last one
standing, described as: **plain outline children of the Main focus root
never paint** — across four focus roots, two navigation paths, with and
without CDC quiescence, on pages `Page`-tagged and not, pre-existing and
freshly created. Held STRONG, not PROVEN: arrangement B's click did not
CHANGE focus (Main was already there — `history_id`/`added_ts` unchanged, no
new `navigation_history` row), so a focus-CHANGING click is formally
untested; residual run is click away to `block:journals`, then back, then
re-read both tables and the paint count. TWO INSTRUMENT ERRORS were caught
and corrected in-lane and are recorded rather than discarded: discriminator
1's lookup returned the wrong block, and this probe's first execution
accepted a 1-element frame as settled so the click was refused (`entity
block:wslice-graft-page not in bounds`) and the comparison never ran — a
`MIN_BOOTED=30` floor fixed it. Superseded reasoning, kept for the record:
the exemplar's own comment (`gpui_journals_viewport.rs:180-183`) argues
navigation origin matters — "the click's bound `navigation.focus` is what
writes `focus_roots`, and the panel renders that table's row" — and
`focus_roots` is a matview over `navigation_history` JOINed to
`navigation_cursor`, so a pre-window focus op could plausibly write a row
the panel does not resolve. Cheap next check before any redesign: dump
`focus_roots` joined to `navigation_cursor` under both arrangements and
diff. So "plain outline children never paint" is at this point a DESCRIPTION
OF THE MEASUREMENTS, not an established product defect; asserting the latter
while axis 2 stands would be the same mistake as the graft-root drift
itself, where a fixture assumption masqueraded as a product symptom. GAP
CLASS, argued rather than assumed: NOT PERCEPTION — a formal assertion
exists and fired (`cmd_enter_chord_dispatch.rs:150`,
`gpui_window_slice.rs:663`); NOT ORACLE — the invariant is present and
correct, it went red the moment it was allowed to run. ENVIRONMENT primary
on the skill's own tiebreak, since navigating to a page IS generatable
headlessly while the failing code path — GPUI layout and element mounting —
does not exist in the keystone's wiring at all, so no headless draw could
ever reach it. COVERAGE secondary under "driver rung not exercised": the
windowed rungs that DO cover this were dark for weeks (they did not compile,
task #24) and are still in no gate, which is why a live paint defect went
unseen. NOT FIXED — production changes are out of this lane's scope; the
four `gpui_window_slice` tests are `#[ignore]`d pointing here, and
`cmd_enter_chord_dispatch` is left red and honest. Probe reverted in full.)

## Missing piece

ENVIRONMENT on the skill's tiebreak: navigating to a page IS generatable
headlessly, but the failing path — GPUI layout and element mounting — does
not exist in the keystone's wiring, so no headless draw could reach it.
COVERAGE secondary under "driver rung not exercised": the windowed rungs
that DO cover this were dark for weeks (task #24) and remain in no gate. NOT
PERCEPTION and NOT ORACLE — a formal assertion exists and fired
(`cmd_enter_chord_dispatch.rs:150`, `gpui_window_slice.rs:663`) the moment
it was allowed to run.

## Remedy

NOT FIXED (production out of lane scope). Both single-variable causes
excluded by measurement: "page never settled" dies on
`block:journals::auto-create` (boot-seeded, settled doc, 59 elements);
"needs a `Page` root" dies because `block:wslice-graft-page` IS
`Page`-tagged via `create_page_block`'s `tags: [PAGE_TAG]`
(`window_slice/seed.rs:409-418`), so the main measurement already covered
that configuration. Fourth cell (`Page` + pre-existing + plain + no query
source) is STRUCTURALLY UNREACHABLE at graft time — the `block_tags` lookup
returned nothing and failed loud in 2.46s, because fixtures graft right
after `start_app` while the `daily_journal` rule fires later in boot;
reaching it is a fixture redesign. CLASSIFICATION HELD OPEN: those probes
vary only ONE of the THREE axes on which this harness differs from the
living exemplar. Axis 3 (the exemplar's `wait_for_cdc_quiescent(500ms,120s)`
after grafting, `gpui_journals_viewport.rs:161-163`) is now ELIMINATED —
adding it to all six graft helpers changed nothing
(`lane-logs/rv-cdcwait.log`, 183.56s, 1258 rounds at 68 elements); reverted.
Axis 2 (pre-window `navigation::focus` vs post-window sidebar CLICK) is
ELIMINATED: navigating to the SAME page both ways yields
`focus_roots`/`navigation_cursor` rows identical field for field, and a
click the driver confirms succeeded still leaves the focus root's plain
child unpainted (64→68 elements, 0 both sides). ALL THREE HARNESS AXES
ELIMINATED, so the product-defect reading stands: **plain outline children
of the Main focus root never paint**, across four focus roots, two
navigation paths, with/without CDC quiescence, `Page`-tagged and not,
pre-existing and fresh. STRONG not PROVEN — B's click did not CHANGE focus,
so a focus-changing click is formally untested (residual: click to
`block:journals` and back). Two instrument errors caught and corrected
in-lane, disclosed. Evidence `lane-logs/rv-discriminator.log`,
`rv-discriminator2.log`, `rv-cdcwait.log`, `rv-axis2.log`; all probes
reverted in full.
