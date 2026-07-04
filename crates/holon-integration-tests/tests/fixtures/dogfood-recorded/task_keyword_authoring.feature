Feature: Authoring a task by typing its keyword into the editable surface

  Recorded live against the GPUI app (sandbox port 8711, SqlOnly / loro=false)
  driving the source-projection editor: the editable surface IS the block's
  vault syntax, so typing "TODO milk" promotes the block and the store
  re-derives content + task_state from the raw text. Born-booted onto the wide
  seed; `c1` starts as "c1", so each scenario empties it with two backspaces
  before authoring.

  WHY THE `Then` IS THIN. The `Then` vocabulary can only match rendered
  substrings — there is no `block "<id>" has task state "TODO"`, no exact
  match at block scope, and no disk assertion. What gives these scenarios
  teeth is the composed catalog, which runs after EVERY step and compares the
  reference model (which carries the task state) against the SUT. A promotion
  landing on the wrong keyword, or a keyword leaking into `content`, diverges
  on the next invariant sweep even though no `Then` names it.

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

  Scenario: A longer word that merely starts with a keyword stays plain text
    When I focus block "block:structural-page" in region "main"
    And I focus the editor of block "block:c1"
    And I press backspace 2 times
    And I type "TODOLIST"
    Then within 10 seconds block "block:c1" contains "TODOLIST"

  Scenario: Deleting the keyword prefix from the surface demotes the block
    When I focus block "block:structural-page" in region "main"
    And I focus the editor of block "block:c1"
    And I press backspace 2 times
    And I type "TODO milk"
    And I press backspace 4 times
    And I type "bread"
    Then within 10 seconds block "block:c1" contains "bread"

