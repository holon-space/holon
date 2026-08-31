---
id: 2026-08-31-mcp-coordinate-click-always-reports-unhandled
date: 2026-08-31
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  The MCP `click{x,y}` tool reports `handled:false` for every click, including
  ones that demonstrably fire a production handler, so the coordinate driver
  cannot answer "did my click land?".
---

## Bug

Found by the `dogfood-explorer` gate driving a real GPUI window over the
embedded MCP server (lane `dogfood-mobile`, port 8720).

Twenty coordinate clicks were issued across two coordinate spaces, including
the exact centres of elements the geometry index reports as painted and
visible. Every one returned:

```json
{"clicked":[…], "button":"left", "handled":false}
```

Among them `{"x":268,"y":121}` — the centre of `selectable#23`, which
`describe_ui` annotates `x=40 y=108 width=456 height=26 has_visible_area=true
match=exact`, and which `click{entity_id}` resolves to the SAME coordinates
(app log: `[ui-event] click_entity(…) coords=(268.0,121.0)`).

The clicks are not being lost. `{"x":150,"y":835}` on the accordion header
reported `handled:false` and yet flipped the accordion from `▸` (collapsed) to
`▾` (expanded) — `shots/05-backlink.png` → `shots/06-accordion-expand.png`.
The production `on_mouse_down` in
`frontends/gpui/src/render/builders/accordion.rs:53-59` ran. Only the reported
verdict is wrong.

## Root cause

Not root-caused from this channel. The MCP side builds its answer from
`response.handled` off the `InteractionCommand` round trip
(`frontends/mcp/src/tools.rs:3949-3957`), whose own comment states the
contract: "`handled:false` means no element consumed the click — nothing was
focused by it." The GPUI side converts the event into a MouseDown + MouseUp
pair (`frontends/gpui/src/lib.rs:3451-3474`) and the entity path dispatches the
identical `InteractionEvent::MouseClick`
(`frontends/gpui/src/user_driver.rs:728`). The `handled` bit is evidently never
set from what the window actually did with the event.

## Missing piece

`click_entity` never reads `handled` — it returns `Ok(())` after the dispatch
and proves focus separately, only for main-region editors
(`user_driver.rs:738-757`). So no test consumes the flag, and its being
permanently false is unobserved. The coordinate form is the ONLY way to reach
a control that carries no entity URI — the chrome sidebar toggle and the
overlay scrim among them (see
`2026-08-31-overlay-sidebar-not-drivable-over-mcp`) — and for exactly those
controls the driver reports failure on success.

## Remedy

Open. Either set `handled` from the window's real disposition of the event, or,
if GPUI cannot report it, fail loud and remove the flag rather than answering
`false` unconditionally — a hard-wired negative is worse than no answer, since
a caller cannot tell a miss from a hit.
