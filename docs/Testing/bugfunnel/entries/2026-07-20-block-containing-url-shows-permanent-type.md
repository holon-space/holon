---
id: 2026-07-20-block-containing-url-shows-permanent-type
date: 2026-07-20
gap: PERCEPTION
secondary: COVERAGE
status: FIXED
summary: >-
  Block containing a URL shows a PERMANENT "Type to search…" popup (user
  report): the `/` slash trigger is registered mid-line
  (`at_line_start:false`, `input_trigger.rs:118`) and matched with
  `text_before_cursor.rfind("/")` (`input_trigger.rs:82`) with NO
  word-boundary/whitespace guard (the `[[` trigger has a closed-link guard at
  `:86`; `/` has none) — every `/` in `https://…/path` fires `command_menu`;
  the URL tail becomes `filter_text`, matches no command → empty items →
  placeholder branch (`editor_view.rs:1767-1774`). Never dismisses:
  `TriggerDismissed` requires NO `/` before the cursor
  (`editor_view_model.rs:305`) — never true inside a URL; only Esc or deleting
  past the last `/` closes it. NOTE dogfood non-repro (static URL block, no
  typing) is consistent: triggers only fire on text change.
source_line: 1044
---

## Bug

Block containing a URL shows a PERMANENT "Type to search…" popup (user
report): the `/` slash trigger is registered mid-line
(`at_line_start:false`, `input_trigger.rs:118`) and matched with
`text_before_cursor.rfind("/")` (`input_trigger.rs:82`) with NO
word-boundary/whitespace guard (the `[[` trigger has a closed-link guard at
`:86`; `/` has none) — every `/` in `https://…/path` fires `command_menu`;
the URL tail becomes `filter_text`, matches no command → empty items →
placeholder branch (`editor_view.rs:1767-1774`). Never dismisses:
`TriggerDismissed` requires NO `/` before the cursor
(`editor_view_model.rs:305`) — never true inside a URL; only Esc or deleting
past the last `/` closes it. NOTE dogfood non-repro (static URL block, no
typing) is consistent: triggers only fire on text change.

## Missing piece

PERC: the empty popup is unobservable — `render_popup` tracks only item rows
in BoundsRegistry (`editor_view.rs:1808`), not the placeholder, and no
snapshot field exposes popup-active/zero-items. COV: content generator never
emits `/`; no transition types URL-like content. Remedy: word-boundary rule
(accept `/` only at line start or after whitespace) + URL-typing transition
+ popup-active-with-empty-items observable, red-first.

## Remedy

FIXED 2026-07-20 (declarative word_boundary on TextPrefix: / accepted only
at line start or after whitespace; dismissal now fires on URL lines; locked
at input_trigger unit + EditorViewModel on_text_changed layers; verifier
CONFIRMED. OPEN residue: per-keystroke popup state still invisible to the
headless keystone — ENVIRONMENT gap unchanged)
