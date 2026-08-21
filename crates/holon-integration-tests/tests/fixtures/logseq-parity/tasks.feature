@wip @core @power @observed
Feature: Tasks and TODO workflow (DB version)
  # Expectations distilled from interaction-log entries 6-10.
  # DB-version tasks = a block tagged with the built-in #Task class carrying
  # Status / Deadline / Scheduled / Priority properties. This DIVERGES from the
  # file version's "TODO " text-marker model.
  #
  # TRIAGE 2026-08-22 (lane gap-props). No scenario in this file is expressible
  # in the current step registry, so all five stay @wip. Holon's task model is
  # the org keyword in the block's `properties` bag (`task_state`), reachable
  # through `I cycle block {id} to state {state}` and through keyword typing on
  # the source-projection surface. Class per scenario below; the vocabulary the
  # class-B ones need is listed once, at the bottom of this comment.
  #
  # MISSING VOCABULARY (blocks tasks 1-3, and already named as "THE ASSERTION
  # IS THE GAP" by tests/fixtures/dogfood-recorded/task_state_cycle.feature and
  # task_keyword_authoring.feature):
  #   - Then `block "<id>" has task state "<state>"` — the read already exists
  #     on the composed cap surface as `SutSqlProjection::block_task_state`.
  #   - Then `block "<id>" is tagged "<tag>"` — `block_tags` is a real junction
  #     table, but the cap surface exposes only `block_tag_block_ids()`, not a
  #     per-block tag read.
  #   - Then an assertion over slash-menu items. The popup lives in
  #     `EditorViewModel` and is rendered by GPUI only; the headless widget tree
  #     the `the widget contains` matcher walks never carries popup items, so
  #     `the widget contains "…"` cannot see the menu.

  @observed
  Scenario: Typing a "TODO " prefix does NOT create a task
    # NOTE: candidate deliberate-deviation (file version auto-converts the prefix)   # log:6
    # TRIAGE: class C — DEVIATION REJECTED, decision-for-review. Holon does the
    # OPPOSITE on purpose and it is already pinned: typing "TODO milk" promotes
    # the block and leaves "milk" as content
    # (dogfood-recorded/task_keyword_authoring.feature, scenario 1), while
    # "TODOLIST" stays plain text (scenario 2). Adopting the DB-version choice
    # would break org round-tripping, which is Holon's storage contract. Keep
    # this scenario as the record of the rejected deviation; it must never go
    # green. Un-@wip only if Martin overturns the org-first task model.
    When I type "TODO Buy milk" into a block and commit
    Then the block renders as plain text "TODO Buy milk"
    And no checkbox or task marker is shown
    And the block is not tagged #Task

  @observed
  Scenario: There is no "/task" slash command   # log:7
    # TRIAGE: class B — the behaviour matches, the assertion cannot be written.
    # Holon's slash menu is DERIVED from the operation registry (each op's
    # `display_name`, plus templates and search — command_provider.rs), so no
    # "/task" command exists here either. Two things are missing: an
    # empty-result affordance (Holon renders no "No matched commands" string —
    # grep finds none) and a Then over popup items.
    When I type "/task" in a block
    Then the slash menu shows "No matched commands"

  @observed
  Scenario: The slash menu uses Node-centric vocabulary   # log:8
    # TRIAGE: class C — DEVIATION REJECTED, decision-for-review. Holon's menu
    # labels are schema-derived from the op registry (ruling #89, landed
    # d8c67fd5; ADR 0024 makes PN the sole action language), so hand-naming
    # entries "Node reference" / "Node embed" under a "BASIC" group would
    # reintroduce the hard-coded vocabulary that ruling removed. Holon's
    # equivalents today are "Embed" and "Delete".
    When I type "/" in a block
    Then the menu offers "Node reference" and "Node embed" under BASIC
    # NOTE: candidate deliberate-deviation (file version calls these block/page references)

  @observed
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

  @observed
  Scenario: Task status is chosen from a Set Status picker   # log:10
    # NOTE: candidate deliberate-deviation (file version cycles TODO/DOING/LATER/NOW markers)
    # TRIAGE: class C — DEVIATION REJECTED for the picker, decision-for-review.
    # Holon cycles "" -> TODO -> DOING -> DONE on a state_toggle click, and the
    # ring is the DOCUMENT's own `#+TODO:` vocabulary rather than a fixed
    # six-status enum (dogfood-recorded/task_keyword_vocabulary.feature). A
    # fixed enum would contradict that per-document vocabulary. A picker that
    # OFFERS the document's declared ring is the compatible variant of this
    # scenario, and is the form worth building if Martin wants one.
    When I click a task's checkbox
    Then a "Set Status" popup lists: Backlog, Todo, Doing, In Review, Done, Canceled
    And each status has a distinct icon and color
    When I choose "Done"
    Then the block shows a green check
    And the node's logseq.property/status is set to Done
