@wip @core @observed
Feature: References and backlinks
  # Expectations distilled from interaction-log entries 11-13.

  @observed
  Scenario: [[ opens page-reference autocomplete, auto-closes brackets, creates on the fly   # log:11
    When I type "Link to [[Project Alpha" in a block
    Then the closing "]]" is auto-inserted
    And an autocomplete popup appears
    And with no existing match it offers "New page Project Alpha"
    When I accept "New page Project Alpha"
    Then a page node is created with name "project alpha" (normalized) and title "Project Alpha"
    And the authoring block stores a reference to that node (not literal text)

  @observed
  Scenario: A page reference renders as a clickable link and navigates   # log:12
    Given a block "Link to [[Project Alpha]]"
    Then "Project Alpha" renders as a colored link with dimmed brackets
    When I click the link
    Then the Project Alpha page opens
    And it is added to the Recent list

  @observed
  Scenario: Linked references list the backlinks grouped by source   # log:13
    Given the Project Alpha page is open
    Then a "Linked references" panel shows the count and the referencing context
    And the reference is grouped under its source page ("2026-08-20")
    And the exact referencing block is shown

  @observed @power
  Scenario: The (( )) block-ref syntax is removed; use [[ ]] for all node refs   # log:19
    # NOTE: candidate deliberate-deviation (file version uses ((uuid)) for block refs)
    When I type "((" to reference a block
    Then a toast appears: "To reference a node, please use `[[]]`."
    And no block-reference autocomplete is shown

  @observed @power
  Scenario: [[ ]] references any node — pages and blocks alike   # log:20, log:21
    When I type "[[This is today" where a block with that content exists
    Then the autocomplete offers that existing block (grouped under its page) and a new-page option
    When I select the existing block
    Then the reference renders as a clickable node link
    And the referenced block shows a numeric reference-count badge
    And clicking the reference navigates/zooms to the target block

  @observed @power
  Scenario: Node embed transcludes a node inline   # log:22
    When I run "/node embed" and pick an existing node
    Then a block renders the target's content transcluded, prefixed with a "→" embed indicator
    And the embed updates live with the source node

  @observed
  Scenario: Unlinked references list plain-text mentions   # log:23
    Given the page title appears as plain text (no [[ ]]) in another block
    When I open the page and expand the "Unlinked references" section (collapsed by default)
    Then the plain-text occurrences are listed grouped by source page with the title highlighted
    And each can be converted into a real link
