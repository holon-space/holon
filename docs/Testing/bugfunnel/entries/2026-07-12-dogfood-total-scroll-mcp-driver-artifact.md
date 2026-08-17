---
id: 2026-07-12-dogfood-total-scroll-mcp-driver-artifact
date: 2026-07-12
gap: PERCEPTION
secondary: null
status: NOTED
summary: >-
  dogfood #3 "P1 total scroll no-op" was an MCP-driver artifact (broken tool),
  NOT the interactive bug — real trackpad scrolls fine; real bug is the
  last-block clip (row above)
source_line: 966
---

## Bug

dogfood #3 "P1 total scroll no-op" was an MCP-driver artifact (broken tool),
NOT the interactive bug — real trackpad scrolls fine; real bug is the
last-block clip (row above)

## Missing piece

explorer drove via the broken MCP scroll tool and over-generalized to "total
no-op"

## Remedy

NOTED — original dogfood #3 scroll row corrected
