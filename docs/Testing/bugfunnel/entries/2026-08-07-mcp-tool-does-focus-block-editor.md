---
id: 2026-08-07-mcp-tool-does-focus-block-editor
date: 2026-08-07
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The MCP `click` tool does not focus the block editor, and reports success
  anyway.
source_line: 1170
---

## Bug

(overnight dogfood-explorer, DRIVER PARITY — this one gates the whole
channel) **The MCP `click` tool does not focus the block editor, and reports
success anyway.** `click {"entity_id":"block:d-l1b","region":"main"}`
returns its self-verifying hit-test
`{"clicked_entity":"block:d-l1b","region":"main"}`, but no caret is placed,
so every MCP-driven click→type sequence is inert and SILENTLY so. Proven by
a paired control, same block, same point, same session: after MCP `click`,
`insert_text` returns `{"inserted_text":" APPENDED","handled":false}` and
`type_text` changes nothing; after a REAL OS-level click (synthesised
`CGEvent` mouse down/up at the same coordinates), the identical
`insert_text` returns `handled:true` and the content changes to match. The
coordinate form `click {x,y}` fails identically.

## Root cause

overnight dogfood, DRIVER PARITY — the MCP `click` tool does NOT focus the
block editor, so every MCP-driven click→type sequence is inert, and the
failure is SILENT: `click` returns a successful self-verifying hit-test
(`{"clicked_entity":"block:d-l1b","region":"main"}`) while no caret is
placed. Proven by control: with focus from MCP `click`, `insert_text`
returns `handled:false` and `type_text` changes nothing; with focus from a
REAL OS-level click at the same point, the identical `insert_text` returns
`handled:true` and the content changes. Coordinate-form `click {x,y}` fails
the same way. This regresses the behaviour the dogfood-explorer skill
documents as verified 2026-07-07 ("a click places the caret and focuses the
editor"), and it silently voids any GPUI/McpUserDriver rung that drives
editing via click)

## Missing piece

This regresses the behaviour the dogfood-explorer skill records as verified
live on 2026-07-07 ("a click places the caret and focuses the editor —
observed caret lands at text end"), so it is a driver regression, not a
documentation gap. Its blast radius is every GPUI/McpUserDriver rung that
reaches an editor via click: those sequences now pass vacuously, asserting
on state no keystroke ever reached. Missing piece = restore the focus
side-effect in the `click` tool AND make it fail loud (it must not answer
with a successful hit-test when it did not focus what it hit); the skill's
launch recipe should meanwhile document the real-CGEvent fallback used to
get this session's results.

## Remedy

**FIXED 2026-08-07** (lane DRIVER-PARITY). Root cause: `element_center`'s
documented first lookup `render-entity-{id}` — the row-wide click-to-focus
wrapper — records NO bounds, so entity-addressed clicks fell through to
`selectable-{id}`, the 16px bullet/drag handle, which never seats a caret.
Fix: `GpuiUserDriver::require_click_center` resolves a main-region click to
the row's TEXT element (`editable_text`/`rendered_text`), and `click_entity`
then WAITS for that editor to take window focus in a committed frame,
DRIVING the frames it paces on via the new `InteractionEvent::ForceFrame`
(the pump calls `window.draw`, which needs neither key status nor
visibility) — `window.refresh()` alone only marks the window dirty, so a
passive wait on a non-frontmost window reported failure on a gesture that
had succeeded, naming a STALE previous holder. Returns `Err` naming the
actual focus holder when the caret genuinely is not seated. Live matrix
(fresh throwaway vault, fresh target block per trial, SQL-confirmed):
frontmost 5/5 pass, NOT frontmost 5/5 pass; inversion (caret clicks resolved
to the bullet again) 5/5 loud errors with `focused_block=None; editors
reporting window focus: []` and `insert_text handled:false` landing nowhere.
