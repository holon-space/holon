@wip @core @power @observed
Feature: Tasks and TODO workflow (DB version)
  # Expectations distilled from interaction-log entries 6-10.
  # DB-version tasks = a block tagged with the built-in #Task class carrying
  # Status / Deadline / Scheduled / Priority properties. This DIVERGES from the
  # file version's "TODO " text-marker model.

  @observed
  Scenario: Typing a "TODO " prefix does NOT create a task
    # NOTE: candidate deliberate-deviation (file version auto-converts the prefix)   # log:6
    When I type "TODO Buy milk" into a block and commit
    Then the block renders as plain text "TODO Buy milk"
    And no checkbox or task marker is shown
    And the block is not tagged #Task

  @observed
  Scenario: There is no "/task" slash command   # log:7
    When I type "/task" in a block
    Then the slash menu shows "No matched commands"

  @observed
  Scenario: The slash menu uses Node-centric vocabulary   # log:8
    When I type "/" in a block
    Then the menu offers "Node reference" and "Node embed" under BASIC
    # NOTE: candidate deliberate-deviation (file version calls these block/page references)

  @observed
  Scenario: Setting a Deadline auto-creates a task with a rich date/repeater picker   # log:9
    When I run the "/deadline" command on an empty block
    Then a picker opens with a calendar, a "Repeat task" toggle,
      | control                | purpose                                       |
      | Every N [Day/Week/Month/Year] | repeat frequency and unit               |
      | Next date advance      | "Advance from scheduled" or "from completion" |
      | When Status is Done    | the repeat trigger condition                  |
      | time-of-day + natural language field | precise / "e.g. Next week" entry |
    When I pick a date
    Then the block gains a checkbox, a red #Task tag, and a "Deadline: <date>" chip
    And the node is persisted with logseq.property/deadline and block/tags -> #Task

  @observed
  Scenario: Task status is chosen from a Set Status picker   # log:10
    # NOTE: candidate deliberate-deviation (file version cycles TODO/DOING/LATER/NOW markers)
    When I click a task's checkbox
    Then a "Set Status" popup lists: Backlog, Todo, Doing, In Review, Done, Canceled
    And each status has a distinct icon and color
    When I choose "Done"
    Then the block shows a green check
    And the node's logseq.property/status is set to Done
