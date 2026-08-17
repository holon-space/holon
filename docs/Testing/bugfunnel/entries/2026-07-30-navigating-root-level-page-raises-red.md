---
id: 2026-07-30-navigating-root-level-page-raises-red
date: 2026-07-30
gap: ORACLE
secondary: PERCEPTION
status: OPEN
summary: >-
  Navigating to any ROOT-LEVEL page raises a red error banner `Breadcrumb
  unavailable: breadcrumb: no Page-tagged ancestors resolved for
  block:kappa-leaf (path "/block:kappa-leaf")`. The block IS `Page`-tagged and
  its parent is `sentinel:no_parent`, so it has zero ancestors by construction
  — the resolver treats "no ancestors" as a failure instead of yielding the
  valid single-element breadcrumb (the page itself). Discriminating control in
  the same session: navigating to the NESTED page `block:alpha-design`, whose
  parent `block:alpha-project` is `Page`-tagged, renders cleanly with no
  banner. The fail-loud disclosure itself is correct per the error-handling
  contract — the defect is the resolution beneath it. Found incidentally while
  dogfooding the sidebar disclosure feature (round 2, rev 0c8f5bbb); NOT
  caused by it — the feature diff touches no breadcrumb or navigation code.
source_line: 1125
---

## Bug

Navigating to any ROOT-LEVEL page raises a red error banner `Breadcrumb
unavailable: breadcrumb: no Page-tagged ancestors resolved for
block:kappa-leaf (path "/block:kappa-leaf")`. The block IS `Page`-tagged and
its parent is `sentinel:no_parent`, so it has zero ancestors by construction
— the resolver treats "no ancestors" as a failure instead of yielding the
valid single-element breadcrumb (the page itself). Discriminating control in
the same session: navigating to the NESTED page `block:alpha-design`, whose
parent `block:alpha-project` is `Page`-tagged, renders cleanly with no
banner. The fail-loud disclosure itself is correct per the error-handling
contract — the defect is the resolution beneath it. Found incidentally while
dogfooding the sidebar disclosure feature (round 2, rev 0c8f5bbb); NOT
caused by it — the feature diff touches no breadcrumb or navigation code.

## Root cause

navigating to any ROOT-LEVEL page raises a red error banner `Breadcrumb
unavailable: breadcrumb: no Page-tagged ancestors resolved for
block:kappa-leaf (path "/block:kappa-leaf")`. The block IS `Page`-tagged and
its parent is `sentinel:no_parent`, so it has zero ancestors by construction
— the resolver treats "no ancestors" as a failure instead of yielding the
valid single-element breadcrumb (the page itself). Discriminating control in
the same session: navigating to the NESTED page `block:alpha-design`, whose
parent `block:alpha-project` is `Page`-tagged, renders cleanly with no
banner. ORACLE primary: the interaction is maximally generatable — the
keystone navigates to root pages constantly — but no invariant anywhere
asserts that breadcrumb resolution SUCCEEDS for a focused page, so every one
of those navigations passes while the banner fires. PERCEPTION secondary: a
red error banner on the most ordinary navigation in the app trains users to
ignore the fail-loud channel, which is the channel's whole value. The
disclosure itself is correct behaviour per the error-handling contract — the
defect is the resolution beneath it. Found incidentally while dogfooding the
sidebar disclosure feature; NOT caused by it — the feature diff touches no
breadcrumb or navigation code.)

## Missing piece

No invariant asserts that breadcrumb resolution SUCCEEDS for a focused page.
The interaction is maximally generatable — the keystone navigates to root
pages constantly — so every one of those navigations passes green while the
banner fires. PERCEPTION secondary: a red error banner on the most ordinary
navigation in the app trains users to ignore the fail-loud channel, which is
that channel's whole value.

## Remedy

OPEN 2026-07-30 — reproduced live over MCP
(`click{entity_id:"block:kappa-leaf", region:"left_sidebar"}` → banner;
`navigation.focus` on a nested page → clean), not fixed. Remedy is an
invariant that a focused page resolves a non-empty breadcrumb, which would
go red on the keystone's existing navigation transitions without any
generator work.
