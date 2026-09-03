---
id: 2026-09-03-breadcrumb-and-window-title-desync-from-the-main-panel
date: 2026-09-03
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  The title bar keeps naming a previously-visited page after the main panel has
  navigated elsewhere.
---

## Bug

Found by exploratory dogfooding (lane `dogfood-explore`) on the live app.

Observed twice, on different navigation routes:

  - main panel showing the shopping view, title bar reading `Compass`
    (`logs/dogfood-session-2026-09-03/03-shopping-view.png`);
  - main panel showing the Compass page, breadcrumb reading
    `Resources > Rezepte > Linsensuppe`
    (`logs/dogfood-session-2026-09-03/06-page-mixed.png`).

In both cases the main panel content was correct and current; only the title-bar
breadcrumb lagged, and it did not self-correct while the page stayed open. The
breadcrumb names the previous destination, so it is actively misleading rather
than merely stale-looking — a user reading it would believe they are somewhere
they are not.

Both navigations were driven through `execute_operation navigation.focus`. The
dogfood skill already records that `navigation.focus` "does not reliably repaint
the main panel"; here the main panel repainted correctly and the title bar was
the surface that did not, which is the same seam observed from the other end.

## Root cause

Not isolated. The breadcrumb is fed by a different path from the main panel's
content and does not observe the same navigation commit, so the two surfaces can
disagree. Whether `navigation.focus` fails to notify the title bar, or the title
bar reads a cursor that is updated later, was not determined in this session.

## Missing piece

No invariant relates the title-bar breadcrumb to the main panel's focus root.
The composed catalog asserts on panel content but the title bar is outside the
inspected ViewModel entirely — it does not appear in `describe_ui` at all (see
`2026-09-03-titlebar-toolbar-is-invisible-to-describe-ui`), so no headless
assertion can currently reach it.

## Remedy

Open. Make the title bar observable in the ViewModel snapshot first; only then
can an invariant require breadcrumb and focus root to name the same block.
