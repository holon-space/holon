@wip @power @documented-only
Feature: Templates
  # From feature-inventory; not driven live in this session.

  Scenario: Define a template
    When I mark a block subtree with a template property/name
    Then it is registered as a reusable template

  Scenario: Insert a template with dynamic variables
    When I insert a template into a block
    Then its subtree is copied in
    And dynamic variables (e.g. current date) are expanded at insertion time
