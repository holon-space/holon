@wip @peripheral @documented-only
Feature: Export and import
  # From feature-inventory; not driven live.

  Scenario: Export a page or graph
    When I export a page
    Then I can produce Markdown / OPML (and graph-level EDN / JSON) output

  Scenario: Publish
    When I publish the graph
    Then a static browsable site is produced
