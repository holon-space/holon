---
id: 2026-08-31-mcp-created-block-never-paints-until-navigation
date: 2026-08-31
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  A block created through `execute_operation block.create` reaches SQL and the
  view model but is never painted, so it stays unclickable until an unrelated
  navigation forces a repaint.
---

## Bug

Found by the `dogfood-explorer` gate driving a real GPUI window over the
embedded MCP server (lane `dogfood-mobile`, port 8720).

```
execute_operation {"entity_name":"block","operation":"create",
  "params":{"id":"block:dogfood-link-1",
            "parent_id":"block:3a2dbaf6-…","content":"child under journal day"}}
-> Operation 'create' on entity 'block' executed successfully
```

The write landed: `SELECT id, content FROM block WHERE id='block:dogfood-link-1'`
returns the row. After ~5s the node is also in the view model —
`grep -c dogfood-link-1` over `describe_ui` returns 8 hits. But it was never
painted, and stayed unpainted across two attempts several seconds apart:

```
click {"entity_id":"block:dogfood-link-1","region":"main"}
-> click_entity failed: element bounds never committed;
   stale focus cleared to prevent silent mis-targeted typing
```

A following `type_text` correctly refused rather than typing into nothing:
`type_text dropped all 19 keystroke(s): no focused editor … consumed them`.

One `navigation.focus` away and back to the same page fixed it — the identical
click then succeeded, and the geometry index went 14 → 37 recorded elements.
So the window paints fine; the create simply never invalidated it.

Both refusals are exemplary fail-loud behaviour and are NOT the bug. The bug is
that the repaint is never scheduled.

## Root cause

Not root-caused from this channel. The write reaches Turso and the reactive
tree (the node is in `describe_ui`), but no window refresh follows, so the
`BoundsRegistry` never commits an element for the new row. The
gesture-originated path does not have this problem — an equivalent edit typed
into an existing block repainted immediately.

Distinct from the screenshot-staleness hazard the `dogfood-explorer` skill
documents: the geometry element count is a live registry read, not a cached
frame, and it stayed at the old value until the navigation.

## Missing piece

Driver parity. The headless composed keystone applies its writes through the
reference/SUT pair and settles; it never asks "did the window commit bounds for
the row this write created?". `McpUserDriver` is the rung where an MCP-origin
write and a gesture-origin write should be indistinguishable, and here they are
not — which matters because `just keystone-mcp` drives the live app exactly
this way, so any case that creates a block and then interacts with it is
walking into an unpainted node.

## Remedy

Open. The gap closes with an invariant that a committed write is followed by a
committed element for the affected row within the settle window, exercised over
the MCP rung — not only over the in-process driver.
