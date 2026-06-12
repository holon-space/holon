Feature: SplitBlock routes prefix to original, suffix to new (composed full_headless)

  Born-booted from the wide seed (structural-page -> parent/c1/c2) over the composed
  `full_headless` CapMap (no `Given org file` / `app is started` ceremony — the doc is
  the boot org). Focus c1, split it at a byte offset; the per-tick
  `inv-block-content-matches-ref` (the composed catalog runs it every step, and it is
  in WIDE_REQUIRED_INVARIANTS so it cannot pass vacuously) catches any prefix/suffix
  mis-routing — the same regression `split_routes_prefix_suffix.feature` guarded over
  E2ESut. The split tail is addressable as the synthetic `block::split-0`.

  Scenario: Splitting a seed block routes prefix to original and suffix to new
    When I focus block "block:structural-page" in region "main"
    And I split block "block:c1" at position 1

  # Exercises the `Then` assert vocabulary over the composed cap surface
  # (`impl FixtureAssertable for ComposedSut` → `focus is on` reads the
  # `current_focus` matview region "main"). Navigating to the page focuses it.
  Scenario: Focus assertion evaluates over the composed SUT
    When I focus block "block:structural-page" in region "main"
    Then focus is on block "block:structural-page"
