---
id: 2026-10-04-slash-command-strip-drops-a-key-queued-behind-enter
date: 2026-10-04
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  A key typed right after a slash-menu gesture is lost: the strip, hide or
  restore of the typed `/command` writes back the buffer read at the gesture.
---

## Bug
Found by code audit in the admission Inc 1a lane, then measured with a
windowed probe. Type `ab /del`, then Enter and `x` with no foreground tick
between them (a key already queued behind the Enter). The buffer ends as
`ab `. The `x` is gone. With a tick between the two keys the buffer is
`ab x` (control).

## Root cause
`apply_popup_action` (`frontends/gpui/src/views/editor_view.rs`) computed the
stripped text from the buffer at Enter time, then applied it with
`set_value` inside `cx.spawn`, on a later foreground tick. A key handled
before that tick was inserted into the live buffer and then overwritten.
The `InsertText`, `CommandFailed`, `HideCommandText` and
`RestoreCommandText` arms had the same shape: `/emb` Enter `proj` with no
tick ends as an empty buffer, and Escape `x` after a search term drops the
`x`.

## Missing piece
The keystone cannot reach this path: the headless mirror routes a slash
command through `slash_command_selection` and dispatches it at once, with no
deferred strip. The windowed slash rungs always ran `run_until_parked`
between keys, so no key was ever queued behind the Enter.

## Remedy
The five arms run their buffer write with `cx.defer`. It runs when the
gesture's effect flush ends, after the window lease is released and before
the next key event, so every queued key acts on the edited buffer. The
expected text of each queued-key rung is the text the same keys give with a
tick after the gesture. An edit of the live buffer on a later tick was not
enough: a queued Backspace, Delete, caret move or selection replace reaches
into the command span, and no text derived from the live buffer equals the
sequential result.
Red then green: `frontends/gpui/tests/slash_command_text_hidden_windowed.rs`
(`a_typed_key_queued_behind_the_command_enter_survives_the_strip`, the
`a_queued_*` rungs, `a_search_term_queued_behind_the_picker_enter_survives_the_hide`,
`a_key_queued_behind_the_picker_escape_survives_the_restore`).
