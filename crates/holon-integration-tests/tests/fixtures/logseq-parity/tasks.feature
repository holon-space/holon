@core @power @observed
Feature: Tasks and TODO workflow (DB version)
  # Expectations distilled from interaction-log entries 6-10.
  # DB-version tasks = a block tagged with the built-in #Task class carrying
  # Status / Deadline / Scheduled / Priority properties. This DIVERGES from the
  # file version's "TODO " text-marker model.
  #
  # RULED 2026-08-26 by Martin (D21.a). Three of the five scenarios recorded
  # LogSeq-DB behaviour Holon deliberately REJECTS. A scenario that can never
  # pass is dead weight, so each was rewritten as its INVERSE: it now asserts
  # HOLON's behaviour and guards the deviation against accidental
  # LogSeq-ification. The LogSeq contrast is kept in each scenario's comment.
  # Scenarios 2 and 4 stay `@wip` — they are absent-feature / missing-oracle
  # gaps, not rejected deviations, and each says which.

  # RULED (D21.a). LogSeq DB: typing a "TODO " prefix must NOT create a task —
  # the marker is plain text. Holon does the OPPOSITE on purpose: the editable
  # surface IS the block's vault syntax, so a declared keyword at the head of
  # the line promotes the block and only the remainder stays as content.
  # Adopting the DB-version choice would break org round-tripping, which is
  # Holon's storage contract.
  #
  # The promotion is asserted with `has task state`, whose oracle is
  # `SutSqlProjection::block_task_state` — the renderer draws a GLYPH and never
  # prints the keyword, so no rendered-substring assertion could say this. The
  # `contains "Buy milk"` arm is the other half: the keyword must be CONSUMED,
  # not left duplicated in `content`.
  #
  # The wide seed's `c1` starts as "c1", hence the two backspaces.
  @observed
  Scenario: Typing a "TODO " prefix DOES promote the block to a task   # log:6
    When I focus block "block:structural-page" in region "main"
    And I focus the editor of block "block:c1"
    And I press backspace 2 times
    And I type "TODO Buy milk"
    Then within 10 seconds block "block:c1" has task state "TODO"
    And within 10 seconds block "block:c1" contains "Buy milk"

  @wip @observed
  Scenario: There is no "/task" slash command   # log:7
    # TRIAGE: class B — the behaviour matches, the assertion cannot be written.
    # Holon's slash menu is DERIVED from the operation registry, so no "/task"
    # command exists here either. What is missing is the EMPTY-RESULT
    # affordance: Holon renders no "No matched commands" string (grep finds
    # none), it renders nothing at all. The menu-item oracle that unblocked
    # scenario 3 below reads the item list, so it can see "the list is empty" —
    # but the recorded `Then` asserts a STRING Holon does not have. Un-`@wip`
    # this by ruling on the empty-menu affordance first, not by weakening the
    # assertion.
    When I type "/task" in a block
    Then the slash menu shows "No matched commands"

  # RULED (D21.a). LogSeq DB hand-names its menu entries "Node reference" /
  # "Node embed" under a "BASIC" group. Holon's labels are SCHEMA-DERIVED from
  # the operation registry — each op's `display_name`, filtered to the ops
  # whose descriptor classifies them `MenuExposure::Listed { slash_menu }`
  # (`CommandProvider::build_command_items`) — per ruling #89 (landed
  # d8c67fd5) and ADR 0024, which makes PN the sole action language.
  # Hand-naming entries would reintroduce exactly the hard-coded vocabulary
  # that ruling removed.
  #
  # The labels below are MEASURED, not chosen: they are what the registry
  # advertises for a block today. That is the point of the gate — if an op's
  # `display_name` or its `menu_exposure` changes, this reds and the change has
  # to be deliberate.
  #
  # The oracle is `SutEditorMirrorRead::editor_slash_menu_labels`, which reads
  # the menu the block's editor actually has open (the `HeadlessEditorMirror`'s
  # own `slash_menus` state, resolved through the SAME
  # `CommandProvider::build_command_items` call the Enter key routes through).
  # The "/" is a real keystroke through the production driver, so the menu is
  # opened the way a user opens it.
  @observed
  Scenario: The slash menu's vocabulary is derived from the operation registry   # log:8
    When I focus block "block:structural-page" in region "main"
    And I focus the editor of block "block:c1"
    # The seed's `c1` is the two chars "c1"; the mid-line "/" trigger is
    # word-boundary gated (it is what keeps a URL's slashes from opening the
    # menu), so the line has to be empty for "/" to fire.
    And I press backspace 2 times
    And I type "/"
    # MEASURED 2026-08-26, the full menu for a seed block:
    #   Indent, Outdent, Move Up, Move Down, Delete Subtree,
    #   Delete Keep Children, Delete, Cycle Task State, Turn into page,
    #   Embed Entity
    # Every one of those is an operation's `display_name`; not one is a
    # hand-written menu string. Four are pinned here — one structural, one
    # destructive, one task, one reference — so a registry change that drops or
    # renames an op cannot pass unnoticed.
    Then the slash menu on block "block:c1" offers "Indent"
    And the slash menu on block "block:c1" offers "Delete"
    And the slash menu on block "block:c1" offers "Cycle Task State"
    And the slash menu on block "block:c1" offers "Embed Entity"
    # The LogSeq-DB labels are ABSENT — this is the deviation being guarded.
    # Note what "Embed Entity" is NOT: it is the `embed` op's own name, not the
    # "Node embed" LogSeq chose, and there is no "Node reference" entry at all.
    And the slash menu on block "block:c1" does not offer "Node reference"
    And the slash menu on block "block:c1" does not offer "Node embed"

  @wip @observed
  Scenario: Setting a Deadline auto-creates a task with a rich date/repeater picker   # log:9
    # TRIAGE: class C — feature absent. Holon has no deadline op, no date
    # picker, and no repeater. The STORAGE half is already there: `deadline`
    # and `scheduled` are org properties that round-trip through the planning
    # line (holon-org-format/src/models.rs:237-251, 796-822). Only the
    # authoring UI is missing; the op-button param popup (landed e929b598) is
    # the nearest host for such a picker.
    When I run the "/deadline" command on an empty block
    Then a picker opens with a calendar, a "Repeat task" toggle,
      | control                | purpose                                       |
      | Every N [Day/Week/Month/Year] | repeat frequency and unit               |
      | Next date advance      | "Advance from scheduled" or "from completion" |
      | When Status is Done    | the repeat trigger condition                  |
      | time-of-day + natural language field | precise / "e.g. Next week" entry |
    When I pick a date
    Then the block gains a checkbox, a red #Task tag, and a "Deadline: <date>" chip
    And the node is persisted with logseq.property/deadline and block/tags -> #Task

  # RULED (D21.a). LogSeq DB cycles a FIXED six-status enum (Backlog, Todo,
  # Doing, In Review, Done, Canceled) chosen from a "Set Status" picker. Holon
  # has no picker, and — more importantly — no fixed enum: which keywords a
  # block may carry is the DOCUMENT's own `#+TODO:` declaration. A fixed enum
  # would contradict that per-document vocabulary.
  #
  # Driven through the authoring path, which is where the document's ring is
  # actually consulted (`editor_source.rs` -> `QueryEngine::block_todo_keywords`):
  # the source projection accepts a keyword the document declares and refuses
  # one it does not. The refusal arm is the DISTINGUISHING half — under a
  # fixed enum "TODO" would promote here, and it must not.
  #
  # NOT covered by this scenario: the state_toggle CLICK cycle. That path binds
  # `col("todo_states")`, which `block_profile.yaml` currently yields as `()`
  # ("no live source of a document's custom TODO keywords" — the `doc:` scheme
  # was retired with ADR 0014), so a click falls back to the default
  # ""/TODO/DOING/DONE ring regardless of the document. Reconnecting the click
  # cycle to the document's ring is its own task; see the note in
  # `assets/default/types/block_profile.yaml`.
  @observed
  Scenario: The document's own #+TODO: vocabulary decides which keywords promote   # log:10
    Given an org file "Vocabulary.org":
      """
      #+TODO: NEXT WAITING | DONE
      * plan
      :PROPERTIES:
      :ID: vocab-declared
      :END:
      """
    When I focus block "block:ref-doc-0" in region "main"
    # A keyword the document DECLARES promotes the block.
    And I focus the editor of block "block:vocab-declared"
    And I press backspace 4 times
    And I type "NEXT ship it"
    Then within 15 seconds block "block:vocab-declared" has task state "NEXT"
    And within 15 seconds block "block:vocab-declared" contains "ship it"

  # The refusal arm of the same ruling, and the DISTINGUISHING half: "TODO" is
  # not in this document's ring, so it must stay plain text. Under LogSeq-DB's
  # fixed status enum — or under any hard-coded ring — it would promote.
  #
  # A separate scenario rather than a second half of the one above, because an
  # editor cannot be opened on a second block while one is active
  # (`FocusEditableText` precondition `active_editor_block().is_none()`) and no
  # step closes one.
  @observed
  Scenario: A keyword the document does NOT declare stays plain text   # log:10
    Given an org file "Vocabulary.org":
      """
      #+TODO: NEXT WAITING | DONE
      * other
      :PROPERTIES:
      :ID: vocab-foreign
      :END:
      """
    When I focus block "block:ref-doc-0" in region "main"
    And I focus the editor of block "block:vocab-foreign"
    And I press backspace 5 times
    And I type "TODO ship it"
    Then within 15 seconds block "block:vocab-foreign" contains "TODO ship it"
    And within 15 seconds block "block:vocab-foreign" has no task state
