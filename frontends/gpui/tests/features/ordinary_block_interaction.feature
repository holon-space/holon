Feature: Ordinary block interaction through a real GPUI window

  Replayed by the `gpui_gherkin_replay` binary against a real GPUI window
  (Full variant, Loro on). Exercises the everyday editing path end-to-end
  through the actual GPUI input pipeline: navigate to a doc, focus a block's
  editor, type via real keystrokes, then indent via the real Tab keymap
  (PlatformInput → keymap → IndentInline → operation dispatch). Assertions
  read the real rendered widget tree.

  Scenario: Focus an editor, type, and indent a sibling
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
    And I focus the editor of block "block:blk-one"
    And I type "Hi"
    Then within 10 seconds block "block:blk-one" contains "Hi"
    When I indent block "block:blk-two"
    Then within 10 seconds block "block:blk-two" contains "SecondBlock"
