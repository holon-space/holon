Feature: Diagnostic — click to focus, then type (real GPUI)

  Narrows whether keystroke mis-routing is specific to FocusEditableText by
  using a real mouse click to move focus before typing.

  Scenario: Click a block then type
    Given an org file "interaction.org":
      """
      * FirstBlock
      :PROPERTIES:
      :ID: blk-one
      :END:
      * SecondBlock
      :PROPERTIES:
      :ID: blk-two
      :END:
      """
    And the app is started with loro
    When I focus block "block:ref-doc-0" in region "main"
    And I click block "block:blk-one" in region "main"
    And I type "Hi"
    Then within 10 seconds block "block:blk-one" contains "Hi"
