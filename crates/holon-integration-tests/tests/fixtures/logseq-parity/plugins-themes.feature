@wip @peripheral @documented-only
Feature: Plugins and themes (API surface)
  # From feature-inventory; category-level only. A theme (Catppuccin) and plugins
  # (todoist) are present in this install's preferences (observed in config).

  Scenario: Install from the marketplace
    When I open the plugin/theme marketplace
    Then I can install plugins and themes that extend UI, commands, and rendering

  Scenario: Configure via config
    Then config.edn / custom.css customize core behavior and appearance
