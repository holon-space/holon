@wip @core @observed
Feature: Slash command menu (DB version)
  # Distilled from interaction-log entries 8, 9, 14 (shots 10, 11, 12, 20).

  @observed
  Scenario: Typing "/" opens a categorized command menu
    When I type "/" in a block
    Then a menu appears with categories BASIC, FORMAT, Heading (and more)
    And BASIC offers "Node reference" and "Node embed"
    And FORMAT offers Link, Image link, Underline, Code block, Quote, Math block
    And Heading offers Normal text and Heading 1..3

  @observed
  Scenario: The menu filters as I keep typing
    When I type "/deadline"
    Then only the "Deadline" command remains
    When I type "/task"
    Then the menu shows "No matched commands"

  @observed
  Scenario: Query commands are available via slash
    When I type "/query"
    Then "Query", "Query function", and "Advanced Query" are offered
