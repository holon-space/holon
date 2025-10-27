Feature: Then assertions on a rendered block widget

  Exercises the Then vocabulary (widget-contains-text) against a block's
  rendered subtree. The root layout references leaf blocks as live_block nodes,
  so block content text lives in the block's own widget tree.

  Scenario: The block widget shows its content after rendering
    Given an org file "assert_demo.org":
      """
      * HelloWorldFooBar
      :PROPERTIES:
      :ID: assert-blk
      :END:
      """
    And the app is started
    When I focus block "block:ref-doc-0" in region "main"
    Then within 10 seconds block "block:assert-blk" contains "HelloWorldFooBar"
    And within 10 seconds focus is on block "block:ref-doc-0"
