---
id: 2026-07-19-daily-note-nesting-mismatch-missing-navigation
date: 2026-07-19
gap: PERCEPTION
secondary: COVERAGE
status: OPEN
summary: >-
  Daily-note nesting mismatch + missing navigation affordances (GPUI dogfood,
  Logseq-parity): (a) typing into the journal's empty "Type here" affordance
  creates new bullets as SIBLINGS of the date page
  (`parent_id=block:journals`), NOT nested under today's `2026-07-19` date
  page, so daily entries land at journals-root next to the date rather than
  under it; (b) after navigating into a page (dangling-link click → Project
  Falcon), the left sidebar shows only the "Integrations" section with no
  page-hierarchy list and there is no visible back/breadcrumb affordance to
  return to Journals.
source_line: 1016
---

## Bug

Daily-note nesting mismatch + missing navigation affordances (GPUI dogfood,
Logseq-parity): (a) typing into the journal's empty "Type here" affordance
creates new bullets as SIBLINGS of the date page
(`parent_id=block:journals`), NOT nested under today's `2026-07-19` date
page, so daily entries land at journals-root next to the date rather than
under it; (b) after navigating into a page (dangling-link click → Project
Falcon), the left sidebar shows only the "Integrations" section with no
page-hierarchy list and there is no visible back/breadcrumb affordance to
return to Journals.

## Missing piece

may be partly by-design (journals = flat page+block list) but violates
daily-note expectations; no invariant models new-block
placement-relative-to-focused-page or the presence of back-navigation; needs
a placement ruling + windowed nav-affordance snapshot

## Remedy

OPEN — found GPUI dogfood 2026-07-19; (a) placement debatable, (b)
navigation gap
