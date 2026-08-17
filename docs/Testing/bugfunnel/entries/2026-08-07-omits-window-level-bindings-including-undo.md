---
id: 2026-08-07-omits-window-level-bindings-including-undo
date: 2026-08-07
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  `list_keybindings` omits the window-level bindings, including undo/redo.
source_line: 1172
---

## Bug

(overnight dogfood-explorer, OBSERVABILITY) **`list_keybindings` omits the
window-level bindings, including undo/redo.** It returns only the 7
structural bindings from `crates/holon-frontend/src/reactive.rs:2008-2036`
(cycle_task_state, indent, join_block, move_down, move_up, outdent,
split_block) and does not report the window-level chords registered at
`frontends/gpui/src/lib.rs:1725-1726` — `cmd-z` undo and `cmd-shift-z` redo
on macOS, `ctrl-z`/`ctrl-y` elsewhere. The dogfood skill's standing rule is
"read a shortcut before you send it — do not assume one", so an agent that
follows the rule correctly concludes undo is unbound and skips the undo/redo
checklist item entirely, which is the opposite of the truth.

## Root cause

overnight dogfood, OBSERVABILITY — `list_keybindings` reports only the 7
structural bindings from `crates/holon-frontend/src/reactive.rs` and OMITS
the window-level ones registered in `frontends/gpui/src/lib.rs:1725-1726`,
including `cmd-z` undo and `cmd-shift-z` redo. The skill's own standing rule
is "read a shortcut before you send it — do not assume one", so an agent
following it concludes undo is unbound and skips the entire undo/redo
checklist item, which is precisely the wrong conclusion. Missing piece = the
snapshot must union both registries)

## Missing piece

An introspection tool that under-reports is worse than one that is absent,
because it converts into a confident wrong answer. Missing piece =
`key_bindings_snapshot()` must union the window-level registry with the
structural one, and the tool should mark which registry each binding came
from so an unbound-looking chord can be told apart from an unreported one.

## Remedy

**FIXED 2026-08-07** (lane DRIVER-PARITY).
`frontends/gpui/src/lib.rs::window_key_bindings()` is now the ONE source for
both `cx.bind_keys` and a registry published into
`DebugServices::window_key_bindings`;
`frontends/mcp/src/keybindings.rs::union_key_bindings` merges it with the
structural snapshot and tags every entry with `registry`
(`structural`/`window`) plus its keymap `context`. A headless session says
so explicitly in `window_registry` instead of presenting a partial list as
complete. Live proof: 22 bindings (7 structural + 15 window) including `undo
→ ["cmd","z"]` and `redo → ["cmd","shift","z"]`.
