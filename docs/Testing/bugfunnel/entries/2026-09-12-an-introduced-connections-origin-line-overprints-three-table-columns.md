---
id: 2026-09-12-an-introduced-connections-origin-line-overprints-three-table-columns
date: 2026-09-12
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  The origin path an introduced connection paints under its name in Settings ›
  Integrations is one unwrapped line that runs across the Config, Status and
  Enabled cells, so all three texts overlap and none of them can be read.
---

## Bug

Found by the `user-connections` dogfood RE-RUN of 2026-09-12, driving the real
GPUI app at `main` `e50f042ba5b3` with a synthetic connection file installed in
a throwaway integrations directory.

A connection introduced by a file paints two disclosure lines under its name in
the Settings › Integrations table: `from <absolute path to the yaml>` and
`calls <hosts>`. The `from` line is laid out as a single unwrapped run of text
starting in the Integration column and continuing straight through the Config,
Status and Enabled columns. The column texts are painted on top of it. In the
capture the three strings `/tmp/holon-dogfood-uc/s2/config/integrations/
fixturebox.yaml`, `unconfigured` and `Pending` are superimposed character over
character and none of them is legible.

The same overlap appears for every introduced connection, whether the loader
admitted it (`fixturebox`) or refused it (`intid`), and whatever the status
word is — the run showing `Sync failing` overprinted the path in exactly the
same way.

The disclosure that the row is supposed to deliver is the path. That is the
half a reader most needs and the half the overlap destroys.

Evidence, all under `/tmp/holon-dogfood-uc/`:

- `s2/shots/09a-fixturebox-row-crop.png` — the row at 1400x860, cropped.
- `s2/shots/10a-row-after-toggle.png` — same row with the switch on.
- `s3/shots/11a-crop.png` — the same overlap with the status word `Sync failing`.
- `s5/shots/16-table-bottom.png` — the whole table; `fixturebox` and `intid`
  both overlap, the six bundled rows are clean.

## Root cause

`crates/holon-app/src/integrations_section.rs` paints `origin` and `hosts`
inside the Integration cell, under the name. The bundled rows carry `origin`
and `hosts` as the empty string, so the extra lines are absent and the cell
keeps its one-word width. An introduced row's `from …` line is far wider than
the 108 px the Integration column resolves to in the 590 px modal content box,
and it neither wraps nor clips at the cell boundary.

`crates/holon-turso/sql/schema/integration_state.sql` gives both columns
`NOT NULL DEFAULT ''`, which is why the defect is invisible for every provider
that ships with the build.

The column widths this collides with are the ones lane `sidecar-sync-fixes`
tuned (`SETTINGS_ITEM_TEMPLATE` `6/5/5/fixed(72)/11` → 108/90/90/72/198 px).
Those widths are correct for the content they were measured against; they were
measured against bundled rows only.

## Missing piece

`frontends/gpui/tests/settings_integrations_table_fits_windowed.rs` asserts
exactly the property that is violated here — every painted descendant of a cell
lies inside that cell. It passes because its fixture contains only bundled
providers, whose `origin` and `hosts` are empty, so no row in it has ever had a
second line in the Integration cell.

The rung needs a row with a non-empty `origin`/`hosts` — that is the whole
shape this feature added, and it is the one shape the geometry rung does not
hold.

## Remedy

FIXED by lane `uc-fixes-2`.

**The pin, red first.** `frontends/gpui/tests/settings_introduced_row_fits_windowed.rs`
— a new windowed rung, because an introduced row is three lines tall and fills
the Settings list's viewport on its own, pushing every bundled row below the
fold; the existing geometry rung and this one cannot judge the same window.
It writes a long origin and hosts into `integration_state` after the modal is
open, so the row arrives through the section's own `live_query`, and asserts
every painted descendant of a cell lies inside it. Red for the right reason:
`column 0 ("Integration") row "integration:claude-history" paints
text-integration:claude-history-origin at x=490.0..903.0, outside its cell's
461.0..569.0`.

**The fix.** Both disclosure lines now truncate. A path is one word with no
break opportunity, so wrapping cannot save it — the cell must shorten what it
holds or overprint its neighbours.

- `crates/holon-frontend/src/shadow_builders/text.rs` gains an `ellipsis` param
  (`"start"` or `"end"`, validated at the DSL build boundary — an unknown
  spelling is refused, not defaulted).
- `frontends/gpui/src/render/builders/text.rs` maps it to gpui's
  `text_ellipsis_start` / `text_ellipsis`.
- `crates/holon-app/src/integrations_section.rs` declares
  `truncate: true, ellipsis: "start"` on the origin and `truncate: true` on the
  hosts.

The origin cuts at the START on purpose: every installed connection shares the
integrations directory, so a path cut at the tail distinguishes nothing, while
`…/fixturebox.yaml` says which file this is. The full path stays whole in the
refusal toast and in the log.

`BoundsRegistry` records a text element's CONTENT, not the glyphs painted after
eliding, so the rung cannot see WHICH end was cut; that choice is stated in the
template's own doc comment instead.

## Attribution

Regression of lane `uc-fixes`, which added the disclosure lines. The geometry
rung it was landed beside held only bundled rows, whose `origin` and `hosts` are
the empty string — so the one shape the feature added was the one shape no
fixture had.
