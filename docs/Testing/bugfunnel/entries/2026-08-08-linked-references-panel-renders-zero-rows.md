---
id: 2026-08-08-linked-references-panel-renders-zero-rows
date: 2026-08-08
gap: PERCEPTION
secondary: ORACLE
status: OPEN
summary: >-
  The Linked references panel renders zero rows on a page with three real
  backlinks, and once collapsed the whole section — header included —
  disappears permanently
source_line: 765
---

## Bug

(dogfood-explorer gate pass) **The Linked references panel renders zero rows
on a page with three real backlinks, and once collapsed the whole section —
header included — disappears permanently**, surviving navigation away and
back with no affordance to reopen it. The main panel's own backlinks query
returns rows[3] against the live DB with that page focused; describe_ui
renders the region as `divider` then `(empty)`.

## Root cause

dogfood-explorer gate pass — **the Linked references panel renders zero rows
on a page with three real backlinks, and once collapsed the whole section
disappears permanently**. The main panel's own backlinks query, run verbatim
against the live DB with the page focused, returns rows[3]; the UI shows the
accordion header and nothing under it, and describe_ui renders that region
as `divider` then `(empty)`. Clicking the header then removes the section
entirely — header included — and it does not return after navigating away
and back, leaving no affordance to reopen it short of an app restart.
Backlinks themselves are correct end to end (matview populated, a coordinate
click on a rendered link moved `focus_roots.main` to `block:journals`), so
this is purely the accordion/live_query render, which no formal invariant
can reach. Secondary ORACLE: rendered-vs-internal divergence of exactly this
shape IS mechanically checkable — the windowed PBT could assert that a
live_query node paints as many rows as its backing SQL returns, which would
have caught it. Evidence:
`docs/Testing/fixture-logs-2026-08-08/dogfood-blank-link-unrepresentable-and-misc.txt`
§2, screenshots `03-journals-backlinks.png` / `05-backlinks-renav.png`)

## Missing piece

Backlinks are correct end to end (matview populated, a click on a rendered
link moved `focus_roots.main`), so this is purely the accordion/live_query
render. The secondary is real and mechanical: a windowed PBT could assert a
live_query node paints as many rows as its backing SQL returns.

## Remedy

**OPEN — reported, not fixed.** Evidence
`docs/Testing/fixture-logs-2026-08-08/dogfood-blank-link-unrepresentable-and-misc.txt`
§2; screenshots `03-journals-backlinks.png`, `05-backlinks-renav.png`.
