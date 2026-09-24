Feature: Joining a plain block into a task lands the caret inside the content

  Recorded live (task #93's fix, confirmed on the real app): backspace at the
  start of a plain block joins it into the TODO task above. The join boundary
  is a CONTENT offset, but the merge target's editable surface now carries the
  keyword prefix, so the seed has to cross it. Live result: the task kept its
  keyword and the next typed character landed in the title ("alpha" + "X" =
  "alphaX") — not the `TODOX` corruption an unshifted seed produces.

  TWO AUTHORING DETOURS, both forced:
  1. `JoinBlock` requires navigation focus to BE the leaf, which no authorable
     step sequence reaches (see split_and_join.feature for the three verbatim
     refusals), so the join is written as the backspace chain the user actually
     performs.
  2. The task above is made a task with `I cycle block … to state "TODO"`
     rather than by typing the keyword, because a scenario may focus only ONE
     editor — a second `I focus the editor of block …` is refused with
     `preconditions FAILED for FocusEditableText`.

  The `Then` can only see the merged title; the composed catalog after every
  step is what compares the task facet and the caret against the reference
  model.

  With the detours above the scenario is the first backspace-join replayed in
  this directory (split_and_join.feature records the join as unreachable). The
  join deletes the seeded `block:c2`; the reference keeps that id in its
  removed-block set, so the `block_history` rows `c2` leaves behind are not
  phantoms.

  Scenario: Emptying the follower of a task and typing into it leaves the task alone
    When I focus block "block:structural-page" in region "main"
    And I cycle block "block:c1" to state "TODO"
    And I focus the editor of block "block:c2"
    And I press backspace 2 times
    And I type "X"
    Then within 10 seconds block "block:c2" contains "X"

  Scenario: The keystroke after a join lands in the task's title
    When I focus block "block:structural-page" in region "main"
    And I cycle block "block:c1" to state "TODO"
    And I focus the editor of block "block:c2"
    And I press backspace 3 times
    And I type "X"
    Then within 10 seconds block "block:c1" contains "c1"
