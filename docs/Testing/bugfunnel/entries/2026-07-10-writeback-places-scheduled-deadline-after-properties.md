---
id: 2026-07-10-writeback-places-scheduled-deadline-after-properties
date: 2026-07-10
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Writeback places SCHEDULED:/DEADLINE: AFTER the :PROPERTIES: drawer —
  invalid org planning-line position (must directly follow the headline);
  Emacs/LogSeq won't parse them there. Values do round-trip into
  `properties.scheduled/deadline`
source_line: 888
---

## Bug

Writeback places SCHEDULED:/DEADLINE: AFTER the :PROPERTIES: drawer —
invalid org planning-line position (must directly follow the headline);
Emacs/LogSeq won't parse them there. Values do round-trip into
`properties.scheduled/deadline`

## Missing piece

round-trip fixture with planning lines missing

## Remedy

FIXED (stream 2026-07-10), two defects: (a) `to_org()` emitted the drawer
before planning lines; (b) deeper — orgize's `planning_node` requires
SCHEDULED+DEADLINE on ONE line (separate lines → second keyword + drawer
spill into body text, silently re-minting the headline id and duplicating
the drawer on round-trip; caught by the existing `round_trip_pbt` suite).
Fix joins both keywords on one planning line directly after the headline.
Test `planning_scheduled_and_deadline_share_one_line_and_round_trip_stably`
