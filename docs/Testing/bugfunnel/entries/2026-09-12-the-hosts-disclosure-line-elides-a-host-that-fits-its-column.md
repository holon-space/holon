---
id: 2026-09-12-the-hosts-disclosure-line-elides-a-host-that-fits-its-column
date: 2026-09-12
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  An introduced connection's `calls` line elides a nine-character host to
  `127.0.` with a third of its column still empty, and when the connection has
  no hosts at all the line paints the bare word `calls` and nothing else.
---

## Bug

Found by the `user-connections` dogfood RE-CHECK of 2026-09-12, driving the
real GPUI app at `main` `060022da56b6`, after the `uc-fixes-2` lane fixed the
origin line's overprint.

The overprint IS fixed: at 1400 and at 800 logical width both disclosure lines
stay inside the Integration cell. What they say is the problem.

The fixture connection's stored `hosts` is `127.0.0.1` — nine characters. The
Integration cell paints `calls 127.0.` followed by an ellipsis. The line above
it, `from …turebox.yaml`, runs about 60% further right in the SAME cell, so the
width to paint `127.0.0.1` whole was there and went unused. A host cut after
`127.0.` names nothing; the line's only job is to say which host the connection
calls.

A refused connection has no hosts at all, and the cell then paints the bare
label `calls` with empty space after it — a label promising a value that never
comes.

At the app's minimum width (300 logical, `MIN_WIDTH` in
`frontends/gpui/src/window_state.rs:24`) BOTH values elide to nothing and the
cell reads `fixturebox` / `from` / `calls`.

Evidence under `scratchpad/dogfood-uc-recheck/shots/`:

- `A-03-hostszoom.png` — `from …turebox.yaml` over `calls 127.0.…`, 1400 wide.
- `B-01-table-800.png` — the same pair at 800 wide.
- `C-04-crop.png` — `from` and `calls` with no values at 300 wide.
- `G-02-settings-short.png` — the bare `calls` on a refused row.

Mirror contents, so the painted text can be compared with the stored value:
`hosts = "127.0.0.1"`, `origin = /private/var/folders/…/fixturebox.yaml`.

## Root cause

`crates/holon-app/src/integrations_section.rs:116` renders the hosts line as
`row(#{gap: 4}, text("calls", …), text(col("hosts"), #{… truncate: true}))`
inside a `flex(6)` column. The origin line one line above is the same shape
with a wider label allowance and `ellipsis: "start"`. Whatever width the
truncating child is allotted, it is measurably narrower than the one the origin
line's child gets in the same cell — the two lines do not share a budget.

The bare-label case is the template's own structure: the label is a literal
`text("calls")` that paints whether or not `col("hosts")` has anything in it.
The `if_col("origin", "", …)` guard one level up switches the whole disclosure
block on the ORIGIN being non-empty, so a row with an origin and no hosts still
gets the `calls` line.

## Missing piece

`settings_introduced_row_fits_windowed.rs` asserts containment and nothing
about legibility, and its fixture `HOSTS` is 46 characters
(`api.fixturebox.example, cdn.fixturebox.example`) — long enough that eliding
it is correct. No case in the tree gives that row a SHORT hosts value, so
"a value that fits is painted whole" has never been asserted, and neither has
"a label is painted only when its value is".

## Remedy

Open. Candidates, in the order I would try them:

1. Give the two disclosure lines one width budget so the short line is not cut
   harder than the long one.
2. Drop the `calls` label when `hosts` is empty, the way `if_col` already drops
   the whole block when `origin` is empty.
3. A windowed pin with a short hosts value asserting the painted text equals
   the stored value — and one with an empty hosts value asserting no bare
   label.

## Fix

Two separate things were wrong and only one of them was the elision.

The bare label is fixed: the `calls` line is now switched on its OWN value
(`if_col("hosts", "", ...)` in `crates/holon-app/src/integrations_section.rs`)
rather than on the origin, so a connection with a file and no hosts paints no
line at all instead of a label with empty space after it.

The `calls 127.0.` cut does NOT reproduce in the harness at the width the
re-check read it at: measured, a nine-character host paints 45px inside a 108px
cell with ~35px to spare, whether or not a long path sits above it. What DOES
reproduce is both values eliding to nothing in a narrow window, and that is the
column collapse recorded in
`2026-09-12-the-settings-integrations-table-collapses-at-the-minimum-window-width`
and fixed by the same `min_width` budget. The rung keeps the standing claim —
the cell must have room to spare for a host this short — so the defect would be
caught if the column's budget were ever spent elsewhere.

Covered by `frontends/gpui/tests/settings_introduced_row_fits_windowed.rs`,
which now SWEEPS 300..1200px through `resize_window` in two row
configurations. Measured across the sweep: the disclosure block is not painted
at all below 640px (the Integration cell is 15px tall with a host and without
one), and the hosts line appears at 900px and above.

- RED `lane-logs/red2-settings_introduced_row_fits_windowed-1789381089.log`.
- GREEN `lane-logs/introduced-sweep-1789390349.log`.
- TEETH `lane-logs/teeth-hosts-settings_introduced_row_fits_windowed-1789390623.log`
  — the `if_col` guard neutered so it never fires; red with
  "the connection stores no hosts and its Integration cell is still 75.0px tall
  against 75.0px"; restored byte-for-byte.
