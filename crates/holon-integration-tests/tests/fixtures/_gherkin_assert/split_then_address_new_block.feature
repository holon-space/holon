Feature: Addressing a SplitBlock-created block (VT1)

  Convention: a block created by SplitBlock is addressable as `block::split-N`,
  where N is the reference model's block counter at split time — 0 for the first
  split in a fresh fixture (WriteOrgFile does not bump that counter). Assertions
  resolve this synthetic id to the real backend id via resolve_ref_block_id, so
  both focus and rendered content can be asserted on the new block.

  Scenario: The new block is addressable and shows the suffix after a split
    Given an org file "vt1.org":
      """
      * HelloWorldFooBar
      :PROPERTIES:
      :ID: vt1-blk
      :END:
      """
    And the app is started
    When I focus block "block:ref-doc-0" in region "main"
    And I split block "block:vt1-blk" at position 10
    Then within 5 seconds block "block::split-0" contains "FooBar"
    And within 5 seconds block "block:vt1-blk" contains "HelloWorld"
