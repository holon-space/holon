Feature: Leaving a freshly promoted task must not fold its keyword into the title

  THIS RECORDS A LIVE DEFECT (dogfood re-entry, task #68) AND THE REASON IT HAS
  NO REGRESSION. Driven on the real GPUI app: an empty block, "TODO x" typed
  into it (the store correctly writes content="x", task_state=TODO), then the
  editor loses focus because another block is clicked. On blur the surface text
  is committed through the CONTENT channel, so `content` becomes "TODO x" while
  `task_state` stays TODO and the org file gains a second keyword:
  `* TODO TODO x`. Reproduced 15/15 across two vocabularies (the default TODO
  ring and a `#+TODO: NEXT WAITING | DONE` page). The operation history names
  the write verbatim: `set_field content "" -> "TODO"`, origin `user`, one op
  after the promotion. The corruption is one-shot: once `content` holds the
  keyword, surface and content column agree again, so a second focus/blur cycle
  is stable.

  NOT RECORDABLE — THE BLUR GESTURE ITSELF. Moving the editor from one block to
  another is the whole defect, and it cannot be authored. A second
  `I focus the editor of block "<id>"` in one scenario is refused verbatim:

    step 4: preconditions FAILED for FocusEditableText — the fixture encodes a
    stale assumption

  and `I click block "<id>" in region "main"` moves NAVIGATION focus, not the
  editor, so it does not blur. There is therefore no authorable step sequence
  in which an editor loses focus to another editor — which is precisely why a
  P1 that reproduces 15/15 by hand has no headless rung. This is the session's
  #1 vocabulary gap; the scenario below is left as a comment so a future
  recorder run can turn it on the moment a blur verb exists.

  # Scenario: Moving the editor to another block leaves the promoted title alone
  #   When I focus block "block:structural-page" in region "main"
  #   And I focus the editor of block "block:c1"
  #   And I press backspace 2 times
  #   And I type "TODO milk"
  #   And I focus the editor of block "block:c2"
  #   Then within 10 seconds block "block:c1" contains "milk"

  Scenario: A promotion that is never blurred keeps the keyword out of the title
    When I focus block "block:structural-page" in region "main"
    And I focus the editor of block "block:c1"
    And I press backspace 2 times
    And I type "TODO milk"
    Then within 10 seconds block "block:c1" contains "milk"
