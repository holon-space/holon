Feature: Authoring a task by typing its keyword into the editable surface

  Recorded live against the GPUI app (sandbox port 8711, SqlOnly / loro=false)
  driving the source-projection editor: the editable surface IS the block's
  vault syntax, so typing "TODO milk" promotes the block and the store
  re-derives content + task_state from the raw text. Born-booted onto the wide
  seed; `c1` starts as "c1", so each scenario empties it with two backspaces
  before authoring.

  The promotion itself is asserted with `block "<id>" has task state "TODO"`
  (oracle: `SutSqlProjection::block_task_state`), which is what separates
  "TODO milk" from "TODOLIST": both leave a plausible `content`, and only the
  keyword read tells the two apart. Still missing: an exact match at block
  scope and a disk assertion, so a keyword LEAKING INTO content on top of a
  correct promotion is still caught only by the composed catalog's per-step
  sweep, not by a `Then` here.

  NOT RECORDABLE — undo after typing. Live, one `cmd+z` unwinds exactly one
  keystroke ("milk" -> "mil"), eleven presses return the block to empty and
  drop the task entirely, and twelve redos restore content "milk" + TODO. The
  scenario cannot be replayed: `And I undo` straight after `I type "TODO milk"`
  is refused verbatim with
  `step 4: preconditions FAILED for UndoLastMutation — the fixture encodes a
  stale assumption`, while the SAME `I undo` after `I split block …` replays
  fine (undo_redo.feature). So undo is recordable after a structural op and
  not after a typing burst — reported as a vocabulary gap.

  Scenario: Typing a declared keyword promotes the block and keeps only the remainder as content
    When I focus block "block:structural-page" in region "main"
    And I focus the editor of block "block:c1"
    And I press backspace 2 times
    And I type "TODO milk"
    Then within 10 seconds block "block:c1" contains "milk"
    And within 10 seconds block "block:c1" has task state "TODO"

  Scenario: A longer word that merely starts with a keyword stays plain text
    When I focus block "block:structural-page" in region "main"
    And I focus the editor of block "block:c1"
    And I press backspace 2 times
    And I type "TODOLIST"
    Then within 10 seconds block "block:c1" contains "TODOLIST"
    And within 10 seconds block "block:c1" has no task state

  # RETITLED 2026-08-22 (lane gv-vocab). The recorded keystrokes do NOT reach
  # the keyword: "TODO milk" is 9 chars, so 4 backspaces leave "TODO " and the
  # block stays promoted. The old title claimed demotion, and `contains "bread"`
  # passed either way — the `has task state` read is what told the two apart.
  # The keystrokes are left exactly as recorded; only the claim is corrected.
  # STILL UNCOVERED: actual demotion (deleting the keyword itself), which no
  # recording in this file performs.
  Scenario: Editing the remainder keeps the block promoted
    When I focus block "block:structural-page" in region "main"
    And I focus the editor of block "block:c1"
    And I press backspace 2 times
    And I type "TODO milk"
    And I press backspace 4 times
    And I type "bread"
    Then within 10 seconds block "block:c1" contains "bread"
    And within 10 seconds block "block:c1" has task state "TODO"

