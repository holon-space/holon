Feature: Creating a block under the focused page and typing into it

  Recorded from a live dogfood session (block creation + typing flow), then
  re-expressed in the derived step vocabulary. Born-booted onto the wide seed
  (structural-page -> parent/c1/c2), so no `an org file` / `the app is started`
  ceremony.

  Scenario: A block created under the focused page carries its content
    When I focus block "block:structural-page" in region "main"
    And I create a block "dogfood created" under the focused page with id "\"block:gen-dogfood\""
    Then within 10 seconds block "block:gen-dogfood" contains "dogfood created"

  Scenario: Typing into a focused editor lands in that block
    When I focus block "block:structural-page" in region "main"
    And I focus the editor of block "block:c1"
    And I type "XY"
    Then within 10 seconds block "block:c1" contains "XY"
