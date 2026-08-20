@wip @power @observed @documented-only
Feature: Tags and classes (DB version)
  # DB version treats tags as CLASSES that can carry properties. Observed via the
  # built-in #Task and #Query classes (log:9, log:15); broader behavior from inventory.

  @observed
  Scenario: Built-in classes render as colored hashtags
    Then a task node shows a red "#Task" tag
    And a query node shows a red "#Query" tag

  @documented-only
  Scenario: A #tag creates or links a tag page/class
    # NOTE: candidate deliberate-deviation vs file-version plain tags
    When I type "#SomeTag" in a block
    Then a class/page "SomeTag" is created or linked
    And the block is tagged with that class

  @documented-only
  Scenario: A class can define properties inherited by its instances
    Given a class with declared properties
    When a node is tagged with that class
    Then the node gains that class's properties for editing and querying

  @hover-revealed @observed
  Scenario: Hovering a tag reveals a remove control   # log:H3
    When I hover a class tag (e.g. "#Task") on a node
    Then an inline "✕" appears to unassign the class from the node
