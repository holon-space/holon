Feature: Indenting and outdenting a sibling block

  Recorded live (tab / shift+tab on a leaf sibling). The structural axis is
  asserted directly: `block X is a child of block Y` reads the write-side store
  snapshot, so a mis-parented block reds HERE and names both ids — not only
  indirectly through the composed catalog's `inv-blocks-match-ref`.

  Seed: `structural-page` → `parent` / `c1` / `c2` as siblings, so indenting
  `c2` re-parents it onto its previous sibling `c1`, and outdenting returns it
  to the page.

  Scenario: Indent then outdent returns the block to its original parent
    When I focus block "block:structural-page" in region "main"
    And I indent block "block:c2"
    Then within 10 seconds block "block:c2" is a child of block "block:c1"
    When I outdent block "block:c2"
    Then within 10 seconds block "block:c2" is a top-level block of "block:structural-page"
    And within 10 seconds block "block:c2" is a child of block "block:structural-page"
    And within 10 seconds block "block:c2" contains "c2"
