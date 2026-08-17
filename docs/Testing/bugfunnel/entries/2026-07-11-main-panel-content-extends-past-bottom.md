---
id: 2026-07-11-main-panel-content-extends-past-bottom
date: 2026-07-11
gap: COVERAGE
secondary: PERCEPTION
status: OPEN
summary: >-
  Main-panel content extends past the bottom of the window and cannot be
  scrolled into view (user report, live vault) — vertical scroll does not
  reach overflow content
source_line: 893
---

## Bug

Main-panel content extends past the bottom of the window and cannot be
scrolled into view (user report, live vault) — vertical scroll does not
reach overflow content

## Missing piece

windowed keystone drives real geometry but generates NO scroll transitions
and has no content-reachability oracle (every projected row must be
reachable via scroll); scroll wiring of the main panel untested

## Remedy

CONFIRMED (dogfood #3, sandbox 70-block page): scroll input is a TOTAL NO-OP
on overflowing tree views — entity- and x/y-targeted, both directions,
magnitudes 800-20000, MD5-identical screenshots; block 70/70 clipped at
viewport edge, trailing creation slot unreachable. Evidence
~/.claude/jobs/ceb646ab/tmp/dogfood3/
