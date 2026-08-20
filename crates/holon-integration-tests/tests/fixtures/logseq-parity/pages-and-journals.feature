@wip @core @observed
Feature: Pages and journals
  # Structurally observed across the session (shots 00-27) plus feature-inventory.

  @observed
  Scenario: The default view is a reverse-chronological journal feed
    Then the Journals view shows the current day page (e.g. "2026-08-20") at top
    And earlier day pages ("2026-08-19") follow below
    And each day page is an independent page identified by its date

  @observed
  Scenario: Referencing a non-existent page creates it lazily
    # log:11 — same mechanism as page references
    When I reference "[[Project Alpha]]" and no such page exists
    Then the page is created on demand and appears in Recent

  @observed
  Scenario: A referenced date creates a date/journal page
    # log:17/26 — the deadline date 2026-08-22 appeared as a page node
    When a Deadline "2026-08-22" is set
    Then a date page "2026-08-22" exists and is searchable

  @documented-only
  Scenario: Page properties
    # From feature-inventory; DB version stores properties as first-class typed nodes.
    When I add properties to a page
    Then they are stored as typed property values on the page node
    And they can be queried and shown as columns in views

  @hover-revealed @observed
  Scenario: Heading hover exposes icon and property controls   # log:H1, H5
    When I hover a journal date heading or a page title
    Then "Add icon" and "Set property" appear above it
    And a journal heading also shows its "#Journal" tag on the right
