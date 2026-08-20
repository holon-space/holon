@wip @power @observed @documented-only
Feature: Properties (DB version — first-class typed values)
  # Observed via task properties (Status/Deadline are typed nodes, log:9-10); the
  # rest from feature-inventory. DB version replaces file-version "key:: value" text
  # with typed property nodes and a property UI.

  @observed
  Scenario: Built-in typed properties back the task model
    Then Status, Deadline, Scheduled, and Priority exist as built-in typed properties
    And a "Show hidden properties" toggle reveals system properties on a node

  @documented-only
  Scenario: User-defined properties are typed
    # NOTE: candidate deliberate-deviation vs file-version free-text "key:: value"
    When I add a property to a node
    Then I choose the property and a value of its declared type (text, number, date, node ref, checkbox, ...)
    And the value is stored as a typed value, not raw text

  @documented-only
  Scenario: Properties can be surfaced as query/table columns
    Given nodes carrying a property
    When a query view includes that property
    Then the property appears as a sortable/filterable column

  @hover-revealed @observed
  Scenario: Hidden properties expand into typed key/value rows   # log:H10
    When I click "Show hidden properties" on a node
    Then hidden props expand as typed rows (key with type icon, value, "---" when empty)
    And hovering a row reveals a leading bullet; the value is click-to-edit
