@wip @core @observed
Feature: Search, command palette, and sidebar navigation
  # Expectations distilled from interaction-log entries 17-18 plus feature-inventory.

  @observed
  Scenario: Cmd+K opens a unified search + command palette   # log:17
    When I press Cmd+K and type "Project"
    Then a palette shows a "Create page called 'Project'" action
    And matching Nodes (pages and blocks) with the query highlighted
    And a Recently-updated list
    And scope Filters: Search only nodes / codes / commands / files / themes
    # Commands are searched from the same palette (no separate command palette)

  @observed
  Scenario: Search results expose open / open-in-sidebar / copy-ref actions   # log:18
    Given the Cmd+K palette has a result selected
    Then the actions available are Open (Enter), Open in sidebar (Shift+Enter), Copy ref (Cmd+C)

  @documented-only
  Scenario: Left sidebar navigation
    # Observed structurally in every screenshot; not individually driven.
    Then the left sidebar shows Journals, Flashcards, Pages, Graph view
    And Favorites and Recent sections
    And the current graph name with a graph switcher at the top

  @documented-only
  Scenario: Right sidebar holds multiple context panels
    # From feature-inventory; opened via Shift+Enter / Shift-click.
    When I Shift-click a page or block reference
    Then it opens as a panel in the right sidebar without leaving the current page
    And multiple panels can be stacked and reordered

  @hover-revealed @observed
  Scenario: Sidebar row hover exposes a context menu   # log:H4
    When I hover a row in the left sidebar (Recent/Favorites/page)
    Then a "⋯" more-actions menu button appears at the right of the row
