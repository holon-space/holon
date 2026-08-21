@wip @power @observed @documented-only
Feature: Tags and classes (DB version)
  # DB version treats tags as CLASSES that can carry properties. Observed via the
  # built-in #Task and #Query classes (log:9, log:15); broader behavior from inventory.
  #
  # TRIAGE 2026-08-22 (lane gap-props). All four stay @wip. Holon HAS tags as
  # first-class edges: a `block_tags(block_id, tag)` junction table, populated
  # from org headline tags and joined by holon-advice's rules
  # (holon-advice/src/lib.rs:196). What Holon does NOT have is any of the three
  # things these scenarios actually test — inline "#tag" authoring, a tag
  # implying a page/class, or a class carrying properties its instances
  # inherit. So the gap is the CLASS half, not the tag half.
  #
  # MISSING VOCABULARY: Then `block "<id>" is tagged "<tag>"`. Only the READ is
  # missing. Tagging a block is already an authorable action — `tags` is an
  # `EdgeField` (holon-api/src/edge_field.rs:48, 176) projected to `block_tags`,
  # and the registry has `I set edge field {update} on block {block_id}`. The
  # cap surface exposes `SutSqlProjection::block_tag_block_ids()` (which blocks
  # appear in the junction at all) but nothing that reads ONE block's tags.

  @observed
  Scenario: Built-in classes render as colored hashtags
    # TRIAGE: class C — DEVIATION REJECTED, decision-for-review. Holon marks a
    # task with a state_toggle GLYPH (open circle for TODO, check for DONE) and
    # keeps the keyword in the org headline; it renders no "#Task" tag, and
    # there is no #Query class at all. Adding a synthetic tag would duplicate
    # state the glyph already shows.
    Then a task node shows a red "#Task" tag
    And a query node shows a red "#Query" tag

  @documented-only
  Scenario: A #tag creates or links a tag page/class
    # NOTE: candidate deliberate-deviation vs file-version plain tags
    # TRIAGE: split. The second Then ("the block is tagged with that class") is
    # class B — the `block_tags` edge exists, only the per-block read and the
    # Then are missing. The first Then (typing "#SomeTag" creates or links a
    # page) is class C: no inline "#" trigger, and a tag does not imply a page.
    # Holon's tags come from org headline syntax, not from inline text.
    When I type "#SomeTag" in a block
    Then a class/page "SomeTag" is created or linked
    And the block is tagged with that class

  @documented-only
  Scenario: A class can define properties inherited by its instances
    # TRIAGE: class C — the single biggest feature gap in this cluster, and the
    # one the rest of the cluster depends on. It needs a tag to BE a declared
    # type and a block to be an INSTANCE of it. Today `DeclareTypedSchema`
    # declares free-standing types that no block can join. This is the natural
    # consumer of block-generalization increment 2 (write authority) — worth
    # designing together with it rather than as parity work.
    Given a class with declared properties
    When a node is tagged with that class
    Then the node gains that class's properties for editing and querying

  @hover-revealed @observed
  Scenario: Hovering a tag reveals a remove control   # log:H3
    # TRIAGE: class C, and not headless-testable even once built. Holon renders
    # no tag chip to hover, and the step registry has no hover verb. A windowed
    # GPUI PBT would be the home for this, not the composed keystone.
    When I hover a class tag (e.g. "#Task") on a node
    Then an inline "✕" appears to unassign the class from the node
