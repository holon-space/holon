Feature: SplitBlock routes prefix to original and suffix to new (SqlOnly)

  Hand-authored happy-path mirror of shrunk_2026-05-19.json. Replayed through
  the same SUT machine + InvBlockContentMatchesRef invariant. A stale block
  :ID: or a wrong split position fails loud (precondition panic / invariant).

  Scenario: Splitting a parser-created block at a byte offset
    Given an org file "split_demo.org":
      """
      * HelloWorldFooBar
      :PROPERTIES:
      :ID: split-target-block
      :END:
      * SecondBlock
      :PROPERTIES:
      :ID: second-block
      :END:
      """
    And the app is started
    When I focus block "block:ref-doc-0" in region "main"
    And I split block "block:split-target-block" at position 10
