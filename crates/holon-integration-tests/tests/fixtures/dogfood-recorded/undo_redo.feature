Feature: Undo and redo after a structural mutation

  Recorded live via the undo/redo MCP tools. The scenario cannot assert the
  thing that historically broke (that undo popped the RIGHT operation, not an
  older one) — there is no negative assertion and no undo-stack observable
  (gaps A6 and A7). What carries the weight is the per-tick composed catalog:
  after `I undo` the reference model and the SUT must agree, so an undo that
  pops the wrong op diverges on the very next invariant sweep.

  Scenario: Undo restores the pre-split content
    When I focus block "block:structural-page" in region "main"
    And I split block "block:c1" at position 1
    And I undo
    Then within 10 seconds block "block:c1" contains "c1"

  Scenario: Redo re-applies the undone split
    When I focus block "block:structural-page" in region "main"
    And I split block "block:c1" at position 1
    And I undo
    And I redo
    Then within 10 seconds block "block:c1" contains "c"
