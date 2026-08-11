Feature: Parentage assertions read the store, not the render

  The structural `Then` vocabulary (gap A1). Both phrasings resolve to the same
  oracle — `SutBackend::block_raw_snapshot().parent_id`, the write-side store
  snapshot the composed parentage invariants read — so a fixture assertion and
  an invariant can never disagree about what the store says.

  Born-booted onto the wide seed: `structural-page` → `parent` / `c1` / `c2`.

  Scenario: The seeded siblings are top-level blocks of the page
    When I focus block "block:structural-page" in region "main"
    Then within 10 seconds block "block:c1" is a top-level block of "block:structural-page"
    And within 10 seconds block "block:c2" is a child of block "block:structural-page"

  Scenario: Indent re-parents onto the previous sibling
    When I focus block "block:structural-page" in region "main"
    And I indent block "block:c2"
    Then within 10 seconds block "block:c2" is a child of block "block:c1"
