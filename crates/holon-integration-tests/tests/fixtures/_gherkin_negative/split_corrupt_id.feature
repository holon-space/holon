Feature: A stale block :ID: fails loud under strict replay (SqlOnly)

  Negative control for the strict runner. The org content declares the split
  target with a typo'd :ID: (`split-target-WRONG`), but the split step still
  references `block:split-target-block`. The block does not exist under that
  id, so the SplitBlock precondition fails and strict replay must HARD PANIC
  — never silently skip the step and report green.

  Scenario: Splitting a block whose :ID: was corrupted
    Given an org file "split_demo.org":
      """
      * HelloWorldFooBar
      :PROPERTIES:
      :ID: split-target-WRONG
      :END:
      """
    And the app is started
    When I focus block "block:ref-doc-0" in region "main"
    And I split block "block:split-target-block" at position 10
