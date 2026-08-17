---
id: 2026-08-07-mcp-tool-never-matches-registered-structural
date: 2026-08-07
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The MCP `send_key_chord` tool never matches any registered structural
  binding.
source_line: 1171
---

## Bug

(overnight dogfood-explorer, DRIVER PARITY) **The MCP `send_key_chord` tool
never matches any registered structural binding.** `send_key_chord
{"entity_id":"block:d-l1b","keys":["tab"]}` answers
`{"action":"none","detail":"No handler matched the key chord"}` — with the
editor genuinely focused by a real OS click, and while `list_keybindings` in
the very same session advertises `tab → indent`. Control at that exact
moment: a REAL `tab` keystroke performed the indent correctly and completely
(block reparented from the page to `block:d-l1a`, `sort_key` 8180 placing it
after its new sibling, its own child following it, and the subsequent `undo`
restoring both parent and sort_key and rewriting the org file correctly). So
prod indent is healthy and the driver is broken.

## Root cause

overnight dogfood, DRIVER PARITY — the MCP `send_key_chord` tool never
matches ANY registered structural binding. `send_key_chord {"keys":["tab"]}`
answers `{"action":"none","detail":"No handler matched the key chord"}` even
with the editor genuinely focused by a real OS click, and even though
`list_keybindings` advertises `tab → indent` in the same session. Control: a
REAL `tab` keystroke at that exact moment performed the indent correctly
(block reparented, subtree followed, sort_key correct). So prod indent is
healthy and the driver is broken — any test rung dispatching structural ops
through `send_key_chord` is exercising nothing)

## Missing piece

Same class and same blast radius as the `click` row: any rung dispatching
structural operations through `send_key_chord` exercises nothing while
reporting a benign `action:none`. Missing piece = route `send_key_chord`
through the same binding registry `list_keybindings` reads, and make an
unmatched chord distinguishable from a chord that matched and no-opped
(today both read as `action:none`).

## Remedy

**FIXED 2026-08-07** (lane DRIVER-PARITY). Root cause: the tool bypassed the
real input pipeline entirely — it called `InputRouter::bubble_input` on the
MCP-side router and dispatched the matched op itself, instead of using the
production `UserDriver::send_key_chord`. Fix: the chord is now looked up in
the union registry `list_keybindings` reports, and a bound chord is pressed
through `UserDriver::send_key_chord` (focus, then real platform key events).
The response carries an enumerated `status`: `executed` / `unbound` (nothing
dispatched, every bound chord listed) / `bound_but_not_dispatched`. Live
proof: `["tab"]` → `status:executed`, `matched:[indent/structural]`, block
reparented; `["f7"]` → `status:unbound`; `["cmd","z"]` →
`matched:[undo/window]` and the indent was undone.
