Feature: SplitBlock content routing across contents and positions (SqlOnly)

  Exercises Background + Scenario Outline + Examples expansion: each data row
  becomes its own fixture (fresh init), substituting <content>, <id>, and <pos>
  into the Background docstring and the split step. All rows replay through
  InvBlockContentMatchesRef.

  Background:
    Given an org file "outline_demo.org":
      """
      * <content>
      :PROPERTIES:
      :ID: <id>
      :END:
      """
    And the app is started

  Scenario Outline: Split <content> at position <pos>
    When I focus block "block:ref-doc-0" in region "main"
    And I split block "block:<id>" at position <pos>

    Examples:
      | content          | id      | pos |
      | HelloWorldFooBar | blk-aaa | 10  |
      | abcdefghij       | blk-bbb | 4   |
      | OneTwoThree      | blk-ccc | 6   |
