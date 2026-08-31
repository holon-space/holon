---
id: 2026-08-31-describe-ui-erases-accordion-subtree
date: 2026-08-31
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  `describe_ui` silently reports the whole accordion subtree as
  `{"widget":"empty"}`, dropping its title, collapse state and every child, so
  no MCP-driven check can observe the accordion at all.
---

## Bug

Found by the `dogfood-explorer` gate while trying to verify the
linked-references accordion over the embedded MCP server (lane
`dogfood-mobile`, port 8720).

The window visibly paints `▾ link Linked references` with a body. The same
moment, `describe_ui{"block_id":"block:root-layout"}` reports for that node:

```json
{ "widget": "empty", "layout_hint": "PinnedToEnd" }
```

No `title`, no `collapsed`, no `max_height_fraction`, no `placement`, and no
children — the `live_query` holding the backlinks is gone from the output
entirely. `grep -c accordion` over the full 314KB dump returns 0, while the
authoring source in the DB
(`block:default-main-panel::render::0`) plainly contains
`accordion(#{title: "Linked references", …})`.

The same erasure applies to any widget name without a `ViewKind` arm.

## Root cause

`ReactiveViewModel::to_view_kind()`
(`crates/holon-frontend/src/reactive_view_model.rs:1380-1486`) matches twelve
widget names — `drawer`, `card`, `chat_bubble`, `collapsible`, `on_hover`,
`bottom_dock`, `op_button`, `live_block`, `live_query`, `render_entity`,
`error`, `loading` — and ends with a catch-all:

```rust
_ => ViewKind::Empty,
```

`ViewKind::Empty` serialises as `"empty"`
(`crates/holon-frontend/src/view_model.rs:547`). `accordion` is built through
`ViewModel::from_widget("accordion", __props)`
(`shadow_builders/accordion.rs:171`) and so falls into the catch-all. There is
no `Accordion` variant: `grep -n Accordion crates/holon-frontend/src/view_model.rs`
returns nothing.

This is a silent degradation, and the sibling constructors do the opposite:
`parse_widget` (`view_model.rs:708`) PANICS on an unknown collection/layout/
element/leaf widget name. Only this generic path swallows it.

## Missing piece

Nothing asserts that every widget a shadow builder can emit survives the
`describe_ui` round trip. `accordion` (and `action_bar`, likewise absent from
the arms) reach production without any oracle noticing they are invisible to
the MCP surface — which is precisely the surface the live-MCP keystone
(`just keystone-mcp`) and this dogfood channel use as their observation
window.

Directly downstream: both accordion defects found in the same session
(`2026-08-31-accordion-paints-one-of-four-backlink-rows`,
`2026-08-31-accordion-starts-expanded-on-first-paint-below-600px`) had to be
established from screenshots and source reading, because the structured
observation channel reports nothing.

## Remedy

Open. Two parts: give `to_view_kind` an `accordion` arm carrying title,
placement, `collapsed` and `max_height_fraction`, and replace the silent
`_ => ViewKind::Empty` with a fail-loud path consistent with `parse_widget`,
so the next unmapped widget is a red test rather than an invisible node. A
round-trip sweep over the shadow-builder registry would pin it.
