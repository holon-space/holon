---
id: 2026-09-01-linkedrefs-divider-survives-empty-accordion
date: 2026-09-01
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  The linked-references accordion hides itself when empty, but the divider()
  authored as its separator is an unconditional sibling, so every page with no
  backlinks paints an orphan horizontal rule under its content.
---

## Bug

Found by `dogfood-explorer` pass #2 over v0.0.23 (`d49ef0316a77`), on the
first-boot journal page of an empty sandbox vault.

The rendered main panel shows the page content, then a full-width horizontal
rule, then nothing. The rule has no section under it. On a fresh vault — no
backlinks anywhere — this is what every page looks like.

`describe_ui block:default-main-panel` confirms the structure: the panel's
top-level children are exactly

```
0 columns   (the page content)
1 divider
2 empty     (PinnedToEnd)
```

The `hide_when_empty` half works — the accordion itself is gone. Only its
separator survives.

This is a regression in the fix that landed as "linked references hide
completely when empty": the section hides, but not *completely* — it leaves its
rule behind.

## Root cause

`block:default-main-panel::render::0` authors the separator as an unconditional
sibling of the accordion rather than as part of it:

```
column(
  columns(#{item_template: live_block()}),
  divider(),
  accordion(#{title: "Linked references", icon: "link",
              max_height_fraction: 0.33, hide_when_empty: true},
            live_query(#{sql: "SELECT bl.* FROM backlinks bl JOIN focus_roots fr ...
                               WHERE fr.region = 'main' ORDER BY bl.content ASC", ...})))
```

`hide_when_empty: true` governs only the `accordion(...)` node. The `divider()`
preceding it is a plain sibling in the same `column`, so nothing suppresses it
when the accordion collapses to nothing.

Evidence: render expression read from the `block` view over MCP;
`describe_ui` output and the boot-frame screenshot at
`/tmp/dogfood2-0901/shots/01-boot.png`.

## Missing piece

No invariant asserts that a `divider()` is followed by a visible sibling — or
more generally, that a hidden section leaves no orphaned chrome behind. The
state is fully reachable headless (it is the default state of any vault with no
backlinks) and the widget tree is observable through the same view-model the
keystone already inspects, so a case reaching it would still have gone green.
That makes this an ORACLE gap, not a perception one: no screenshot is needed to
see it, only an assertion nobody wrote.

The `hide_when_empty` feature was pinned by whatever test covers the accordion,
and that test evidently asserts the accordion's own absence without asserting
the absence of its separator.

## Remedy

Open. Proposed:

1. Make the separator part of the section it separates, so one `hide_when_empty`
   governs both — the structural fix, and the one that cannot rot.
2. Add an invariant rejecting a trailing/orphaned `divider()` (a divider with no
   following visible sibling) in a rendered panel. Red today on any page with no
   backlinks.

Note the third child, `empty` with `layout_hint: PinnedToEnd`, is the action-bar
slot and correctly paints nothing; it is not the cause.
