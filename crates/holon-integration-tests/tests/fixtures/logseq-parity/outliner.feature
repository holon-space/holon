@wip @core @observed
Feature: Outliner editing
  # Expectations distilled from interaction-log entries 1-5 (LogSeq DB version, HolonTest graph).
  # LogSeq's blocks are called "nodes" in the DB version; behavior below is the outline tree.

  Background:
    Given a journal page with a block "This is today"

  @observed
  Scenario: Enter creates a sibling block   # log:2
    When I place the caret at the end of "This is today" and press Enter
    And I type "Sibling 1"
    Then a new sibling bullet "Sibling 1" appears below at the same indent level
    And "Sibling 1" is persisted as its own node with a block/order and a parent = the journal page

  @observed
  Scenario: Tab indents the current block under the preceding sibling   # log:3
    Given sibling blocks "Sibling 1" and "Sibling 2"
    When the caret is in "Sibling 2" and I press Tab
    Then "Sibling 2" becomes a child of "Sibling 1"
    And "Sibling 2" block/parent now points to "Sibling 1"
    And pressing Shift+Tab restores it to a sibling

  @observed
  Scenario: Collapse hides a subtree and marks the parent   # log:4
    Given "Sibling 1" has a child "Sibling 2"
    When I click the disclosure caret left of the "Sibling 1" bullet
    Then the child subtree is hidden
    And a right-pointing triangle and a halo ring appear on the "Sibling 1" bullet
    And the collapsed state is persisted (block/collapsed? = true)
    And clicking the caret again expands the subtree

  @observed
  Scenario: Zoom-in re-roots the view to a block   # log:5
    When I click the bullet dot of "Sibling 1"
    Then the view re-roots so "Sibling 1" is the page root showing its subtree
    And a breadcrumb trail to the parent page is shown above
    And a Home control appears to exit the zoom
    And no content is modified (navigation only)

  @hover-revealed @observed
  Scenario: Block-row hover exposes bullet, drag handle, and collapse triangle   # log:H2
    When I hover a block row
    Then the bullet and a drag handle are shown
    And a parent block also shows its collapse/expand triangle
