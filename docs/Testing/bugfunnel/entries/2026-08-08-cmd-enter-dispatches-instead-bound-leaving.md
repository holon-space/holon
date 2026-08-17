---
id: 2026-08-08-cmd-enter-dispatches-instead-bound-leaving
date: 2026-08-08
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  cmd+enter dispatches `split_block` instead of the `cycle_task_state` it is
  bound to, leaving a junk empty block in the document, and `send_key_chord`
  reports the binding it did NOT run as `executed`.
source_line: 761
---

## Bug

(dogfood-explorer gate pass over main e1ba76e6, real GPUI app over its
embedded MCP) **cmd+enter dispatches `split_block` instead of the
`cycle_task_state` it is bound to, leaving a junk empty block in the
document, and `send_key_chord` reports the binding it did NOT run as
`executed`.** One isolating chord produced `2 execute_operation:
entity=block, op=split_block` and zero `cycle_task_state`; the `cmd`
modifier is dropped and `enter` falls through to its own structural binding.
The operation itself is sound via `execute_operation`, and
tab/shift+tab/enter/backspace/cmd+z all resolve correctly on the same path.

## Root cause

dogfood-explorer gate pass over the day's main (e1ba76e6), driving the real
GPUI app over its embedded MCP — **cmd+enter dispatches `split_block`
instead of the `cycle_task_state` it is bound to, and the driver reports the
binding it did NOT run as `executed`**. `list_keybindings` and
`send_key_chord` both advertise
`{"action":"cycle_task_state","chord":["cmd","enter"],"registry":"structural"}`;
the isolating probe sent exactly one cmd+enter and the dispatcher logged `2
execute_operation: entity=block, op=split_block` and zero
`cycle_task_state`, leaving a junk EMPTY block in the user's outline
(`parent=block:49f0614c…`, `sort_key="8180"`) that survives an app restart
and paints as a stray "Type here" row. The `cmd` modifier is dropped and
`enter` falls through to its own structural binding. ENVIRONMENT, not
COVERAGE: the keystone drives `cycle_task_state` as an OPERATION and its
oracle is fine — the chord→action resolution layer only exists in the
windowed frontend, so no headless draw can reach it; the control shows the
operation itself is sound (`execute_operation block/cycle_task_state` →
`task_state=TODO`, `task_state_category=active`, disk `* TODO Alpha one`)
and that tab/shift+tab/enter/backspace all resolve correctly on the same
path, as does cmd+z (it dispatched `restore_join`, correctly undoing the
junk split). Secondary and self-inflicted: `send_key_chord` returning
`{"status":"executed","matched":[cycle_task_state]}` for an action that
never ran means any windowed PBT asserting on that reply is asserting on a
lie — the reply must report what DISPATCHED, not what MATCHED. Same surface,
same class: coordinate `click` returns `"handled":false` while demonstrably
seating the caret (every mid-text split in this session was driven that
way). Evidence verbatim:
`docs/Testing/fixture-logs-2026-08-08/dogfood-cmd-enter-fires-split-not-task-cycle.txt`.
**BOTH HALVES FIXED 2026-08-08 (task #11).** The PERCEPTION half went first,
because without it the other half's red cannot be expressed at the MCP
surface: `DispatchJournal` (`crates/holon-frontend/src/dispatch_journal.rs`,
recorded at all three real `dispatch_intent*` entry points and reachable
through `BuilderServices::dispatch_journal`) records what actually reached
the dispatcher, and `send_key_chord` now brackets the press with a journal
mark and answers with BOTH `matched` (keymap) and `dispatched` (truth), with
`status` = `executed` only when an op the chord matched actually ran, the
new `dispatched_other_action` when something else ran, and
`bound_but_not_dispatched` when nothing did — a session whose services
expose no journal now ERRORS instead of repeating the match as an outcome.
ROOT CAUSE of the chord half, and it is NOT a dropped modifier:
`gpui_component` binds `enter`, `shift-enter` and `secondary-enter` to ONE
action, `input::Enter { secondary }`, so GPUI parses the modifier off the
keystroke INTO the action payload — but both of Holon's `Enter` capture
handlers (`frontends/gpui/src/views/editor_view.rs:1069`,
`render_entity_view.rs:146`) asked `window.modifiers().platform`, ambient
state maintained by separate `ModifiersChanged` events that no simulated
press emits. Every driver-originated cmd+enter therefore read "no cmd held"
and took the plain-Enter arm. FIX: both sites read `enter.secondary`. Class,
not special case — these were the only two ambient-modifier reads in the
frontend, and the fix removes the dependence on ambient modifier state
entirely rather than teaching one chord about cmd; DISCLOSED asymmetry,
since it changes the honest scope: a HARDWARE cmd+enter very likely worked
(macOS does send `ModifiersChanged`), so the escape was specific to every
simulated/driver path — which is exactly the path all windowed automation
uses. Red-first windowed rung
`frontends/gpui/tests/cmd_enter_chord_dispatch.rs::cmd_enter_cycles_task_state_and_plain_enter_still_splits`,
RED with `cmd+enter must dispatch cycle_task_state; it dispatched
["split_block"]`, green after, and it asserts the junk-block symptom
directly (block count unchanged on cmd+enter, +1 on plain enter, so neither
count assertion is vacuous). Red/green verbatim:
`docs/Testing/fixture-logs-2026-08-08/cmd-enter-chord-red-then-green.txt`.
**AMENDED after lane verification, which REFUTED the first version of the
PERCEPTION fix**: the journal recorded only `dispatch_intent*`, so the 15
WINDOW-registry chords
(undo/redo/open_search/cycle_tab_*/jump_to_tab_1..9/turn_into_page) — whose
handlers call `FrontendSession::undo` and friends directly and never reach
the dispatcher — produced an empty press window, and the new reply answered
`bound_but_not_dispatched` ("nothing ran") for a cmd+z that ran and mutated
the outline. Wrong in a NEW way, so: every window handler now journals under
its registry action name (`DispatchJournal::record_window_action`), entries
carry an OUTCOME (`Pending`/`Succeeded`/`Failed(msg)`) settled at the seams
that already knew the result, `executed` means the matched action ran AND
succeeded, `dispatched_but_failed` carries the verbatim error,
`dispatched_outcome_pending` is reported rather than assumed good, and
presses are serialized behind `DebugServices::key_chord_press` because
attribution is press-window-based (now stated in the tool description). The
classification is a pure function, `holon_mcp::key_chord_report::classify`,
unit-tested per registry and per outcome (7/7). Load-bearing proof for the
window half is a MUTATION: blanking the one `record_window_action` line reds
the windowed test with `cmd+z must journal the window-registry \`undo\`
action; the press window held []` — the refuted behaviour, reproduced as a
failure. Evidence:
`docs/Testing/fixture-logs-2026-08-08/send-key-chord-window-registry-truthfulness.txt`.
NOT fixed here, still open on the same surface, and now TWO of them: (1)
coordinate `click` returning `"handled":false` while seating the caret; (2)
**click-to-focus does not seat a caret on a main-panel row in either
driver** — `SimUserDriver` under TestPlatform (`send_key_chord: click on
block:chord-target never moved focused_block to it within 4s (incl. re-click
attempts)`, on a row that had demonstrably painted) and the real
`GpuiUserDriver` against a live windowed instance (`"block:4dca296c-…"'s
editable_text never took window focus within 5s … editors reporting window
focus: []`, with the window fronted and `HOLON_GPUI_FORCE_ACTIVE=1`).
Distinct from the 2026-08-07 targeting fix. Blast radius: the driver REFUSES
to press when it cannot seat a caret, so in a non-interactive session
`send_key_chord` errors for EVERY chord and no live reply can be captured at
all — which is why this lane's coverage is a windowed test plus a
unit-tested classifier rather than a live capture.)

## Missing piece

The chord→action resolution layer exists only in the windowed frontend, so
no headless draw can reach it; the keystone drives `cycle_task_state` as an
operation and never as a keystroke. Compounding it, the driver's reply
reports what MATCHED rather than what DISPATCHED, so a windowed PBT built on
it would assert on a lie (same class: coordinate `click` returns
`"handled":false` while demonstrably seating the caret).

## Remedy

**OPEN — reported, not fixed (this lane reports only).** Needs a windowed
PBT that sends cmd+enter and asserts the task state changed,
red-for-the-right-reason first. Evidence
`docs/Testing/fixture-logs-2026-08-08/dogfood-cmd-enter-fires-split-not-task-cycle.txt`.
