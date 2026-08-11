Feature: Indenting and outdenting a sibling block

  Recorded live (tab / shift+tab on a leaf sibling). Deliberately assertion-free
  on the structural axis: the `Then` vocabulary has no way to say "block X is a
  child of block Y" (gap A1 — the single most-hit gap of the session). The
  scenario still has teeth: the composed invariant catalog runs after EVERY
  step, so a mis-parented block reds on `inv-blocks-match-ref`.

  Scenario: Indent then outdent returns the block to its original parent
    When I focus block "block:structural-page" in region "main"
    And I indent block "block:c2"
    And I outdent block "block:c2"
    Then within 10 seconds block "block:c2" contains "c2"
