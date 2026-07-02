Feature: Ordinary block interaction through a real GPUI window

  Replayed by the `gpui_gherkin_replay` binary through the composed windowed
  path (`with_windowed_wide_sut` + `replay_steps` over `ComposedSut<WideE2E>`).
  Re-authored POST-BOOT onto the wide seed (structural-page -> parent/c1/c2):
  no `Given an org file` / `app is started` ceremony — the wide seed IS the
  boot org (the same convention as the headless composed_split_gherkin
  fixture). Gestures ride the window's SimUserDriver (click-intent resolution
  over real rendered bounds) and the composed invariant catalog runs every
  tick inside `replay_steps`.

  Scenario: Click to focus blocks through the real window
    When I focus block "block:structural-page" in region "main"
    And I click block "block:c1" in region "main"
    # `focus is on` reads current_focus(main) — page-level navigation focus, which a
    # child click does not move (the click focuses the child's editor; the per-tick
    # inv-window-focus-matches-engine-focus invariant checks that side).
    Then focus is on block "block:structural-page"
