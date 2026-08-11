Feature: Opening a page and navigating history

  Recorded live: a click on a sidebar page row navigates the main panel to that
  page. The LIVE half of that flow is NOT reproduced here — see the note below —
  so what remains is the history half, which the wide seed does support.

  NOT RECORDABLE (environment, not vocabulary): the sidebar click itself.
  `When I click block "block:structural-page" in region "left_sidebar"` parses
  cleanly through the registry, but the replay hard-panics at step 0:

      [fixtures:gherkin "Clicking a page row in the left sidebar focuses that
      page"] step 0: preconditions FAILED for ClickBlock — the fixture encodes
      a stale assumption

  The wide seed renders no page rows in the left sidebar, so no sidebar row is
  clickable in the composed headless SUT. The vocabulary is adequate; the seed
  is not.

  Scenario: Focusing a page in the main region, then navigating back
    When I focus block "block:structural-page" in region "main"
    And I navigate back in region "main"

  Scenario: Focus follows an explicit main-region navigation
    When I focus block "block:structural-page" in region "main"
    Then within 10 seconds focus is on block "block:structural-page"
