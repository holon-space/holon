Feature: Cycling a block through the task-state keywords

  Recorded live: cmd+enter on a block cycles "" -> TODO -> DOING -> DONE, and the
  keyword reaches all three surfaces (properties JSON `task_state` /
  `task_state_category`, the rendered state_toggle glyph, and the org headline
  on disk). NOTE: task-keyword PROMOTION semantics are mid-change in a sibling
  lane, so only the plain cycle is recorded.

  THE ASSERTION IS THE GAP. The `Then` vocabulary has no way to say
  `block "<id>" has task state "TODO"` — it can only match rendered substrings,
  and the renderer does NOT print the keyword (it draws a glyph: an open circle
  for TODO, a check for DONE, per the live screenshot). So the only honest
  `Then` here is that the block's CONTENT survives the cycle. What gives the
  scenario teeth is the composed catalog running after every step: the reference
  model carries the task state, so a cycle landing on the wrong keyword diverges
  on the next invariant sweep even though no `Then` mentions it.

  Scenario: Cycling a leaf block to TODO leaves its content intact
    When I focus block "block:structural-page" in region "main"
    And I cycle block "block:c1" to state "TODO"
    Then within 10 seconds block "block:c1" contains "c1"

  Scenario: Cycling forward through DOING and DONE
    When I focus block "block:structural-page" in region "main"
    And I cycle block "block:c1" to state "TODO"
    And I cycle block "block:c1" to state "DOING"
    And I cycle block "block:c1" to state "DONE"
    Then within 10 seconds block "block:c1" contains "c1"

  Scenario: Clearing the state returns the block to plain text
    When I focus block "block:structural-page" in region "main"
    And I cycle block "block:c1" to state "DONE"
    And I cycle block "block:c1" to state ""
    Then within 10 seconds block "block:c1" contains "c1"
