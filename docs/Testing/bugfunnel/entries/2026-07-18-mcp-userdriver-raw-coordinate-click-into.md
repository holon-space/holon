---
id: 2026-07-18-mcp-userdriver-raw-coordinate-click-into
date: 2026-07-18
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  MCP UserDriver raw-coordinate click into empty space below the last block
  hung the MCP request >120s and wedged `/health` until killed (entity-based
  clicks unaffected). The coordinate branch of the `click` tool
  (`frontends/mcp/src/tools.rs:2630`) sends
  `InteractionEvent::MouseClick{position:(x,y)}` and blocks on
  `resp_rx.await`, which never resolves when the click lands on no
  hit-testable element — an unbounded await with no timeout (found during live
  slash-menu verification on Mac)
source_line: 806
---

## Bug

MCP UserDriver raw-coordinate click into empty space below the last block
hung the MCP request >120s and wedged `/health` until killed (entity-based
clicks unaffected). The coordinate branch of the `click` tool
(`frontends/mcp/src/tools.rs:2630`) sends
`InteractionEvent::MouseClick{position:(x,y)}` and blocks on
`resp_rx.await`, which never resolves when the click lands on no
hit-testable element — an unbounded await with no timeout (found during live
slash-menu verification on Mac)

## Missing piece

the keystone drives via entity-based clicks (`click_entity`), never the
raw-coordinate MCP `MouseClick` wiring, so the empty-space hit-test miss +
unbounded `resp_rx.await` is absent from the test env; needs a timeout /
fail-loud on the coordinate click path plus a rung exercising a coordinate
click that hits nothing

## Remedy

OPEN — found live 2026-07-18; the raw-coord click path has no response
timeout
