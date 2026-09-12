---
id: 2026-09-12-a-running-consent-flows-progress-line-leaves-the-setup-column
date: 2026-09-12
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  While an integration's consent flow is running, the progress sentence beside
  its Setup buttons is painted 24 px past the Setup cell and onto the modal
  panel's border, and the windowed rung that judges that cell looks only at the
  operation buttons, so it passes.
---

## Bug

Found by agent measurement inside the `wrap-prop` lane (ruling D120.a item 2),
not by a failing test. While adding the render-DSL `wrap` keyword that makes the
Setup column's operation buttons wrap, the same cell's OTHER child — the
`configure_progress` sentence — was checked and turned out to leave the column.

An integration whose consent flow is in flight carries the sentence
`Waiting for you to finish in the browser.` in `integration_state.
configure_progress`, and the Settings › Integrations Setup cell paints it beside
the operation buttons. Measured in a real window at 560x900 over a booted
engine, with the mirror carrying that sentence for `claude-history`
(probe log `lane-logs/probe-1789245272.log` of the `wrap-prop` workspace):

| element | painted x | Setup cell x |
|---|---|---|
| `text-integration:claude-history-configure_progress` | 372.5 .. 543.0 | 366.5 .. 519.0 |

The sentence is 170.5 px wide in a 152.5 px column. It runs 24.0 px past the
cell's right edge and reaches 543.0, which is the modal panel's own right border
at 544.0 — so the tail of the sentence is painted on the border and cut off. The
user cannot read the end of the only line that says what the flow is waiting
for.

The operation buttons in the same cell were correct in that same capture
(`Switch integration` at 366.5..438.5, `Open` wrapped onto a second line at
366.5..400.5): the wrap keyword landed on the horizontal `list` that holds the
buttons, and the sentence is not in that list.

Only the 560 px window was measured. The desktop width was not.

## Root cause

The Setup cell is
`row(#{gap: 6, align: "center"}, list(#{… horizontal: true, wrap: "wrap"}),
text(col("configure_progress")))`
(`crates/holon-app/src/integrations_section.rs:127`). Two flex items: the
wrapping button cluster, and the sentence.

The `row` builder makes a plain nowrap flex row
(`frontends/gpui/src/render/builders/row.rs`). A flex item's automatic minimum
is its min-content width, and the sentence declares no `truncate`, so it claims
the width it wants and pushes past the container. Wrapping the button cluster
did not help it: `ItemFlow::WrappingRow` is read by
`frontends/gpui/src/render/builders/column.rs::eager_collection_div`, which
governs the collection's own line breaking and nothing about its siblings.

## Missing piece

An ORACLE gap, not a coverage one. The state is reachable and was reached:
`settings_setup_column_wraps_windowed` painted the overflowing sentence in the
probe run and still reported `test result: ok`. Its three claims are scoped to
elements whose contract id starts with `op-button-`, so the sibling text in the
same cell is invisible to it. `settings_integrations_table_fits_windowed` does
judge every descendant of every cell, but its fixture leaves
`configure_progress` empty on every row — the column has never held this string
under any rung.

So: no invariant judges the non-button content of a Setup cell, and no fixture
puts a running consent flow on screen.

## Remedy

Open. Candidate fix: a `wrap` prop on the `row` builder — the same keyword and
the same `flex_wrap()` mapping the horizontal collection now has — so the
sentence moves to a line below the buttons instead of past the column. Truncation
is the wrong answer here: unlike the origin path, this sentence is prose whose
tail carries the instruction.

The fix must come with its own red first: widen
`settings_setup_column_wraps_windowed`'s claims from `op-button-*` to every
painted descendant of the Setup cell, and seed a running consent flow, so the
rung goes red for this geometry before the prop lands.

The keystone cannot reproduce it: this is painted geometry in a real window, and
`general_e2e_composed_pbt` has no window. It belongs to the windowed tier, which
is where the rung above already lives.
