---
id: 2026-09-03-titlebar-toolbar-is-invisible-to-describe-ui
date: 2026-09-03
gap: PERCEPTION
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Search, settings, theme and the rest of the title-bar toolbar appear in no
  describe_ui tree and respond to no entity-addressed click, so the whole
  toolbar is unreachable by any automated driver.
---

## Bug

Found while trying to exercise search and settings from the dogfood-explorer
lane against the live app.

`describe_ui` on `block:root-layout` returns the sidebar drawer and the main
panel and nothing else. The title-bar toolbar — search, settings, theme, the
sidebar toggles and the debug entries visible in every screenshot — has no node
in that tree. Neither does the search overlay once open: with the overlay
plainly visible on screen and accepting keystrokes, `describe_ui` of the root
layout still contains no search node.

Consequences measured this session:

  - `send_key_chord` cannot open search. It requires an `entity_id` and refuses
    to press unless that entity seats a caret, so the window-level `cmd-k`
    binding (`list_keybindings` reports `open_search` bound to `cmd k`) is not
    reachable through it.
  - `execute_command` requires a `block_id` and `list_commands` on the root
    layout returns `[]`, so no command route reaches the toolbar either.
  - Coordinate clicks do reach it, but report `"handled":false` whether or not
    they hit — the reply cannot be used to tell a hit from a miss. Search was
    eventually opened by a coordinate click whose own reply said it was not
    handled, and this was discovered only from a screenshot.
  - The settings surface was never opened at all, so its secret-masking
    behaviour could not be verified in this run.

The whole toolbar is therefore verifiable only by screenshot, which is what let
`2026-09-03-quick-open-search-returns-no-matches-for-every-query` — a total
functional failure of a headline feature — sit undetected.

## Root cause

The toolbar and the search overlay hold their state outside the reactive
ViewModel that `describe_ui` serializes (`frontends/gpui/src/search_ui.rs:61-85`
for the overlay), so the introspection surface has nothing to report. The
coordinate-click path reports `handled` from a hit test that does not consult
these elements either.

## Missing piece

The dogfood and windowed-test surfaces silently exclude an entire region of the
window. Nothing declares that exclusion, so an agent reads an empty
`describe_ui` as "not present" rather than "not observable" — a perception gap
that hides every defect behind it.

## Remedy

Open. Bring the toolbar and overlay into the ViewModel snapshot so
`describe_ui` sees them and `click`/`send_key_chord` can address them by
entity. Failing that, make the omission loud: `describe_ui` should name the
regions it does not cover, and the coordinate-click reply must distinguish a
hit from a miss instead of always reporting `handled:false`.
