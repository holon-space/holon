@power @documented-only
Feature: Templates
  # From feature-inventory; the definition half is not driven live yet.

  # GAP (class B): marking an arbitrary subtree as a template has no step
  # vocabulary — the composed harness only seeds the ONE canned template
  # (`template_fixture.rs`: `block:tpl` / `block:tpl-c1`) as part of the
  # `InstantiateTemplate` transition.
  @wip
  Scenario: Define a template
    When I mark a block subtree with a template property/name
    Then it is registered as a reusable template

  # Un-`@wip`ed 2026-08-25. The canned template is `{{date}}` with one child
  # `see {{date}} now` (`template_fixture.rs`); instantiating under `c1` with
  # fixed bindings expands `{{date}}` at insertion time — the original's
  # "dynamic variables (e.g. current date) are expanded". The `{{mood}}`
  # binding is declared (`TPL_VARS`) but unused by the canned content, so
  # only the date expansion is observable here.
  #
  # The instance block ids below are not magic: production mints them
  # deterministically (`holon_api::effect_id::deterministic_instance_id`) as
  # UUIDv5(HOLON_TEMPLATE_NAMESPACE, "block:tpl\x1f<ctx>\x1f<node>") with
  # ctx = "pbt:block:c1:2026-08-25:happy" (the transition's `context_key`)
  # and node = `block:tpl` / `block:tpl-c1`. Change the parent, date, or mood
  # and the ids change with them.
  Scenario: Insert a template with dynamic variables
    When I instantiate a template under block "block:c1" for date "2026-08-25" with mood "happy"
    # "its subtree is copied in": the instance root lands under the target
    # block and keeps the definition's root→child shape.
    Then within 10 seconds block "block:30738eb0-212c-58c5-97c9-7f8cde4e6883" is a child of block "block:c1"
    And within 10 seconds block "block:70a03b8e-dac6-5ba1-8388-b30f0854e3c2" is a child of block "block:30738eb0-212c-58c5-97c9-7f8cde4e6883"
    # "dynamic variables are expanded at insertion time": the stored instance
    # content carries the bound date value, not the `{{date}}` slot.
    And within 10 seconds block "block:30738eb0-212c-58c5-97c9-7f8cde4e6883" contains "2026-08-25"
    And within 10 seconds block "block:70a03b8e-dac6-5ba1-8388-b30f0854e3c2" contains "see 2026-08-25 now"
