Feature: Splitting a block (and why the join half could not be recorded)

  Recorded live: Enter mid-text splits, Backspace-at-start joins. Only the split
  half is here.

  NOT RECORDABLE — `JoinBlock` is unreachable from any authorable step sequence.
  Three replay rounds, three different refusals, each verbatim:

    1. `When I focus block "block:structural-page" in region "main"`
       + `And I join block "block:c2" with its predecessor`
       -> step 1: preconditions FAILED for JoinBlock
       (`JoinBlock` requires main focus to BE the leaf: `Reason::FocusedIsNotSelf`,
        crates/holon-integration-tests/src/pbt/transitions/join_block.rs)

    2. `When I focus block "block:c2" in region "main"` (make the leaf the focus)
       -> step 0: preconditions FAILED for NavigateFocus
       (`NavigateFocus` only accepts focus roots / pages, not leaves)

    3. `And I arrow-navigate "down" 3 times in region "main"` (walk to the leaf)
       -> step 1: preconditions FAILED for ArrowNavigate

  `ClickBlock` is not a fourth option: it moves EDITOR focus, not navigation
  focus (stated in composed_split_gherkin's own comment). So the single most
  ordinary destructive gesture in an outliner has no recordable scenario. This
  is the session's #1 vocabulary gap — see the report.

  Scenario: Split routes prefix to the original and suffix to the new block
    When I focus block "block:structural-page" in region "main"
    And I split block "block:c1" at position 1
    Then within 10 seconds block "block:c1" contains "c"
    And within 10 seconds block "block::split-0" contains "1"
