@wip @peripheral @documented-only
Feature: Sync and mobile
  # From feature-inventory; a cloud icon is present in the top toolbar (observed).

  Scenario: Logseq Sync
    When Sync is enabled for a graph
    Then changes propagate across devices, optionally end-to-end encrypted

  Scenario: Mobile behavior
    Then the mobile app provides the same graph with touch-oriented outlining and a quick-capture flow
