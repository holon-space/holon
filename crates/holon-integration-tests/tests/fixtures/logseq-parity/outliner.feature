@core @observed
Feature: Outliner editing
  # Expectations distilled from interaction-log entries 1-5 (LogSeq DB version, HolonTest graph).
  # LogSeq's blocks are called "nodes" in the DB version; behavior below is the outline tree.
  #
  # Executable scenarios are re-expressed against the wide seed
  # (`block:structural-page` -> `parent` / `c1` / `c2` as top-level siblings) in the
  # derived step vocabulary, the way `tests/fixtures/dogfood-recorded/*.feature` are.
  # The journal-page framing of the original recording is not load-bearing for the
  # outline-tree behaviour these assert; the scenarios still tagged `@wip` need
  # vocabulary (or functionality) Holon does not have yet — each says which.
  #
  # CONVENTION — what "I split" / "I indent" actually drive here: in the composed
  # HEADLESS SUT these route through op dispatch (`split_block` / `indent` via
  # `cap_transition!` -> `op_write_cap.rs`), NOT through the named keystroke. The
  # keystroke pipeline (click -> focus -> Enter/Tab, `split_block.rs:67`) is the
  # WINDOWED arm. So the scenario titles name the LogSeq gesture, but what is
  # gated below the title is the operation it dispatches. A regression in chord
  # resolution or the key-event path would NOT red these — that is the windowed
  # keystone's job.

  @observed
  Scenario: Enter creates a sibling block   # log:2
    # LogSeq: Enter at end of a block opens a new sibling at the same indent level,
    # persisted as its own node with a parent = the page. Here: split at the end of
    # `c1` (content "c1", length 2) is the Enter, and the sibling is addressable as
    # `block::split-0`. Same-indent-level == same parent as `c1`, asserted against
    # the write-side store.
    #
    # DROPPED FROM THE ORIGINAL (class-B gap, same kind as the three `@wip`
    # scenarios below): sibling ORDER. The original asserted the new bullet
    # "appears BELOW" `c1` and carries a `block/order`; nothing here — or anywhere
    # in the corpus — asserts that `block::split-0` sorts AFTER `c1`. The whole
    # `Then` vocabulary is ParentIs / WidgetContains / FocusOn
    # (`src/pbt/fixtures/matchers.rs`), which has no ordering matcher. Parentage
    # is asserted; position among siblings is NOT. A regression that re-parents
    # correctly but slots the new block in the wrong place stays green here.
    When I focus block "block:structural-page" in region "main"
    And I split block "block:c1" at position 2
    And I type "Sibling 1"
    Then within 10 seconds block "block::split-0" contains "Sibling 1"
    And within 10 seconds block "block::split-0" is a top-level block of "block:structural-page"
    And within 10 seconds block "block:c1" contains "c1"

  @observed
  Scenario: Tab indents the current block under the preceding sibling   # log:3
    # DROPPED FROM THE ORIGINAL: the caret precondition. The original opened with
    # `When the caret is in "Sibling 2"`; here `c2` is never focused and no editor
    # is opened on it, so this scenario engages NO editor mirror — it is a pure
    # tree-shape assertion. Indent-while-editing is therefore still uncovered.
    # Sibling order is likewise unasserted after the outdent (see the note above).
    When I focus block "block:structural-page" in region "main"
    And I indent block "block:c2"
    Then within 10 seconds block "block:c2" is a child of block "block:c1"
    When I outdent block "block:c2"
    Then within 10 seconds block "block:c2" is a top-level block of "block:structural-page"
    And within 10 seconds block "block:c2" contains "c2"

  @wip @observed
  Scenario: Collapse hides a subtree and marks the parent   # log:4
    # GAP (class B): the collapse ACTION exists (`I toggle collapse of {target_id}`,
    # `I toggle the expander of block {block_id}`), but no `Then` vocabulary can
    # observe the outcome: there is no negative widget matcher (`block X does not
    # contain "…"` / `the subtree of X is hidden`) and no matcher for a persisted
    # `collapsed` flag. The bullet's triangle + halo ring are GPUI-only pixels.
    Given "Sibling 1" has a child "Sibling 2"
    When I click the disclosure caret left of the "Sibling 1" bullet
    Then the child subtree is hidden
    And a right-pointing triangle and a halo ring appear on the "Sibling 1" bullet
    And the collapsed state is persisted (block/collapsed? = true)
    And clicking the caret again expands the subtree

  @wip @observed
  Scenario: Zoom-in re-roots the view to a block   # log:5
    # GAP (class C): Holon has no zoom-in affordance on an arbitrary block.
    # `I focus block {block_id} in region {region}` (NavigateFocus) is a LeftSidebar
    # PAGE-row click — its preconditions require `predicts_navigation_focus(...,
    # LeftSidebar)`, so a plain text block cannot become the main-panel root.
    # Breadcrumb rendering exists (frontends/gpui/src/breadcrumb.rs) but has no
    # matcher; `I navigate home in region "main"` would serve as the Home control.
    When I click the bullet dot of "Sibling 1"
    Then the view re-roots so "Sibling 1" is the page root showing its subtree
    And a breadcrumb trail to the parent page is shown above
    And a Home control appears to exit the zoom
    And no content is modified (navigation only)

  @wip @hover-revealed @observed
  Scenario: Block-row hover exposes bullet, drag handle, and collapse triangle   # log:H2
    # GAP (class B): production HAS the affordance
    # (frontends/gpui/src/render/builders/on_hover.rs, and `expand_toggle.rs`
    # reveals its chevron on hover), but the step vocabulary has no hover
    # transition and no `UserDriver` hover gesture, and hover-revealed pixels are
    # outside the headless widget-tree snapshot. Windowed (GPUI) PBT territory.
    When I hover a block row
    Then the bullet and a drag handle are shown
    And a parent block also shows its collapse/expand triangle
