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
    # The original's "appears BELOW `c1`" is the last `Then`: the split product
    # sorts AFTER `c1` among the page's children. The oracle is
    # `SutSqlProjection::sorted_children` (`sort_key`, the authoritative
    # fractional index) — the same order source `inv-live-children-match-ref`
    # compares. A regression that re-parents correctly but slots the new block
    # in the wrong place reds here.
    When I focus block "block:structural-page" in region "main"
    And I split block "block:c1" at position 2
    And I type "Sibling 1"
    Then within 10 seconds block "block::split-0" contains "Sibling 1"
    And within 10 seconds block "block::split-0" is a top-level block of "block:structural-page"
    And within 10 seconds block "block:c1" contains "c1"
    And within 10 seconds block "block::split-0" comes after block "block:c1"
    # The wide seed's top-level order is `parent`, `c1`, `c2`, so `c1` is the
    # SECOND child; the split product is slotted directly behind it.
    And within 10 seconds block "block:c1" is child 2 of block "block:structural-page"
    And within 10 seconds block "block::split-0" is child 3 of block "block:structural-page"

  @observed
  Scenario: Tab indents the current block under the preceding sibling   # log:3
    # DROPPED FROM THE ORIGINAL: the caret precondition. The original opened with
    # `When the caret is in "Sibling 2"`; here `c2` is never focused and no editor
    # is opened on it, so this scenario engages NO editor mirror — it is a pure
    # tree-shape assertion. Indent-while-editing is therefore still uncovered.
    When I focus block "block:structural-page" in region "main"
    And I indent block "block:c2"
    Then within 10 seconds block "block:c2" is a child of block "block:c1"
    When I outdent block "block:c2"
    Then within 10 seconds block "block:c2" is a top-level block of "block:structural-page"
    And within 10 seconds block "block:c2" contains "c2"
    # Outdent puts `c2` back BEHIND the sibling it was indented under, not at
    # the front of the page — without this the round trip could land it anywhere.
    And within 10 seconds block "block:c2" comes after block "block:c1"
    # Re-parenting is not folding: neither end of the indent/outdent round trip
    # may leave the block that briefly gained a child marked collapsed.
    And within 10 seconds block "block:c1" is not collapsed
    And within 10 seconds block "block:c2" is not collapsed

  # The third `Then` of log:4: an authored `:COLLAPSED: t` reaches the store as
  # typed document state. The fold fields are observed by the `/matview` arm of
  # `inv-blocks-match-ref` — the `/block_raw` arm compares only
  # {Content, Properties, Marks}. Background:
  # docs/Testing/bugfunnel/entries/2026-08-22-org-ingest-drops-collapsed-into-property-bag.md,
  # 2026-08-22-loro-create-projection-drops-fold-state.md and
  # 2026-08-22-sql-authority-org-ingest-loses-fold-state.md.
  @observed
  Scenario: A folded block carries its collapsed mark into the store   # log:4
    Given an org file "Folded.org":
      """
      * Folded parent
      :PROPERTIES:
      :COLLAPSED: t
      :ID: folded-parent
      :END:
      ** Hidden child
      :PROPERTIES:
      :ID: hidden-child
      :END:
      * Open sibling
      :PROPERTIES:
      :ID: open-sibling
      :END:
      """
    When I focus block "block:ref-doc-0" in region "main"
    Then within 15 seconds block "block:folded-parent" is collapsed
    And within 15 seconds block "block:open-sibling" is not collapsed
    And within 15 seconds block "block:hidden-child" is a child of block "block:folded-parent"

  # The gesture half of log:4. `I toggle collapse of` clicks the row's REAL
  # disclosure caret (ruling 2026-08-25): the driver routes the
  # `expand_toggle::<id>` handle to `UserDriver::click_expand_toggle`, which
  # clicks the chevron geometry (windowed) or runs the located caret node's
  # own click handler (headless) — a true toggle, so it drives BOTH the fold
  # and the unfold. Collapse is document state; both caret shapes dispatch
  # `set_field(collapsed)` through the dispatcher.
  @observed
  Scenario: Clicking the disclosure caret folds the subtree   # log:4
    When I focus block "block:structural-page" in region "main"
    And I indent block "block:c2"
    Then within 10 seconds block "block:c2" is a child of block "block:c1"
    When I toggle collapse of "block:c1"
    Then within 10 seconds block "block:c1" is collapsed
    When I toggle collapse of "block:c1"
    Then within 10 seconds block "block:c1" is not collapsed

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
