Feature: Cycling a block through the task-state keywords

  Recorded live: cmd+enter on a block cycles "" -> TODO -> DOING -> DONE, and the
  keyword reaches all three surfaces (properties JSON `task_state` /
  `task_state_category`, the rendered state_toggle glyph, and the org headline
  on disk). NOTE: task-keyword PROMOTION semantics are mid-change in a sibling
  lane, so only the plain cycle is recorded.

  The keyword itself is asserted directly with `block "<id>" has task state
  "TODO"`, whose oracle is `SutSqlProjection::block_task_state`
  (`json_extract(properties, '$.task_state')` on `block_raw`) — the SAME read
  `inv-task-state-storage-coherence` compares against the Loro projection. The
  renderer does NOT print the keyword (it draws a glyph: an open circle for
  TODO, a check for DONE, per the live screenshot), so a rendered-substring
  `Then` could never have said this; the content assertions below stay as the
  guard that the keyword does not leak into `content`.

  Scenario: Cycling a leaf block to TODO leaves its content intact
    When I focus block "block:structural-page" in region "main"
    And I cycle block "block:c1" to state "TODO"
    Then within 10 seconds block "block:c1" contains "c1"
    And within 10 seconds block "block:c1" has task state "TODO"

  Scenario: Cycling forward through DOING and DONE
    When I focus block "block:structural-page" in region "main"
    And I cycle block "block:c1" to state "TODO"
    Then within 10 seconds block "block:c1" has task state "TODO"
    When I cycle block "block:c1" to state "DOING"
    Then within 10 seconds block "block:c1" has task state "DOING"
    When I cycle block "block:c1" to state "DONE"
    Then within 10 seconds block "block:c1" has task state "DONE"
    And within 10 seconds block "block:c1" contains "c1"

  Scenario: Clearing the state returns the block to plain text
    When I focus block "block:structural-page" in region "main"
    And I cycle block "block:c1" to state "DONE"
    Then within 10 seconds block "block:c1" has task state "DONE"
    When I cycle block "block:c1" to state ""
    Then within 10 seconds block "block:c1" has no task state
    And within 10 seconds block "block:c1" contains "c1"
