@wip @power @observed
Feature: Queries (DB version — visual filter builder + table views)
  # Expectations distilled from interaction-log entries 14-16.
  # DB-version simple queries are a structured filter builder rendering a live
  # table VIEW, not a {{query}} text DSL. Advanced (datalog) queries remain available.

  @observed
  Scenario: /query offers three query kinds   # log:14
    When I type "/query"
    Then the menu lists "Query", "Query function", and "Advanced Query"

  @observed
  Scenario: A simple Query is composed with a visual filter builder   # log:15
    # NOTE: candidate deliberate-deviation (file version writes {{query (and ...)}} text)
    When I insert a "Query"
    Then a block tagged #Query with a "+ Filter" builder is created
    When I click "+ Filter"
    Then I can add filters by dimension:
      | Tags | Page reference | Property | Task | Priority | Page | Full text search | between | Sample |
    And I can combine them with the operators and / or / not

  @observed
  Scenario: Query results render as a live, configurable table view   # log:16
    Given a #Query block
    When I add a Task filter and select the status "Done" and Apply
    Then the filter chip reads "task: Done"
    And a "Live query" result renders as a table with columns Name, Tags, Status, Deadline
    And the table has a toolbar for sort, filter, search, and view-layout switching
    And only nodes matching the filter appear as rows

  @documented-only
  Scenario: Advanced (datalog) query
    # From feature-inventory; "Advanced Query" accepts a datalog/datascript query
    # with :query, :inputs, :result-transform, and rendering options.
    When I insert an Advanced Query with a datalog expression
    Then the raw datascript query is executed against the graph
    And the result set is rendered per the view/result options

  @hover-revealed @observed
  Scenario: Query-row hover exposes open controls   # log:H6
    When I hover a row in a Live query table
    Then a "→" open-node button and a "▭" open-in-sidebar button appear in that row

  @hover-revealed @observed
  Scenario: Query column headers are configurable   # log:H8
    When I click a Live query column header
    Then a column-config menu opens (sort, pin, property name/type, available choices, checkbox mapping, UI position, hide)
