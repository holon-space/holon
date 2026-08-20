@wip @peripheral @documented-only
Feature: Whiteboards
  # From feature-inventory; not driven live.

  Scenario: Create a whiteboard canvas
    When I create a whiteboard
    Then I get an infinite canvas for shapes, text, connectors, and portals

  Scenario: Embed nodes as portals
    When I drag a page/block onto the canvas
    Then it embeds as a live portal linked to the underlying node
