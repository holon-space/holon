Feature: A document's own task-keyword vocabulary governs its keywords

  Recorded live: a page declaring `#+TODO: NEXT WAITING | DONE` cycles through
  exactly that ring (NEXT -> WAITING -> DONE -> cleared -> NEXT, verified in
  properties and in the org headline on disk), and a block carrying an
  UNDECLARED keyword survives being edited — the surface refuses to project
  vault syntax it cannot parse back, pins the block to the content channel and
  discloses the refusal at WARN.

  NEITHER HALF IS FULLY RECORDABLE, and the two reasons are the session's top
  vocabulary gaps:

  1. A `#+TODO:`-declaring document cannot be authored. `Given an org file
     "<name>":` + docstring is refused whenever the org content leads with a
     `#+…` line — verbatim: `step "an org file \"custom_vocab.org\":" matches
     the WriteOrgFile template but its fields do not parse: org-file step "an
     org file \"custom_vocab.org\":" needs a docstring holding the org
     content`. So every scenario below runs under the DEFAULT vocabulary, and
     the custom ring is reported in the session report only.
  2. Putting a block into an UNDECLARED state needs
     `set_field(task_state, …)`; there is no set-field verb, and
     `I cycle block … to state …` can only reach states the ring offers. The
     refusal arm was therefore driven over MCP only.

  What IS recorded is the default ring and the fact that a cycle leaves the
  block's own text alone — the composed catalog carries the task facet.

  Scenario: Cycling walks the ring and leaves the title alone
    When I focus block "block:structural-page" in region "main"
    And I cycle block "block:c1" to state "TODO"
    And I cycle block "block:c1" to state "DONE"
    And I cycle block "block:c1" to state ""
    Then within 10 seconds block "block:c1" contains "c1"
