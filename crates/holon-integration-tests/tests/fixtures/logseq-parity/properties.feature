@wip @power @observed @documented-only
Feature: Properties (DB version — first-class typed values)
  # Observed via task properties (Status/Deadline are typed nodes, log:9-10); the
  # rest from feature-inventory. DB version replaces file-version "key:: value" text
  # with typed property nodes and a property UI.
  #
  # TRIAGE 2026-08-22 (lane gap-props). All four stay @wip. The load-bearing
  # fact for this whole file: Holon's typed-datatype axis (DeclareTypedSchema /
  # CreateTypedEntity, landed 2026-08-21) produces FREE-STANDING entities —
  # "no parent, no tree, no org projection", its own `<type>_raw` table plus a
  # read matview (transitions/create_typed_entity.rs). A BLOCK cannot be given
  # a declared type, so it does not carry typed properties. LogSeq-DB's model —
  # a node tagged with a class whose properties are typed — therefore does NOT
  # map onto the typed-datatype work; treating it as the same thing would
  # shoehorn two different features together.
  #
  # What Holon has instead: an untyped JSON `properties` bag on `block`, queried
  # with `json_extract(properties, '$.<key>')` (holon-turso/src/sql_utils.rs:277,
  # matview_manager.rs:1210). `task_state`, `deadline`, and `scheduled` all live
  # there. So the DATA is queryable per property; the TYPES and the property UI
  # are what is absent.

  @observed
  Scenario: Built-in typed properties back the task model
    # TRIAGE: class C — partly present, untyped. `task_state`, `deadline`, and
    # `scheduled` are real org properties in the block's bag; Priority is not
    # modelled, and none of the four is TYPED or declared as built-in. There is
    # no hidden-property concept, so the second Then has nothing to bind to.
    Then Status, Deadline, Scheduled, and Priority exist as built-in typed properties
    And a "Show hidden properties" toggle reveals system properties on a node

  @documented-only
  Scenario: User-defined properties are typed
    # NOTE: candidate deliberate-deviation vs file-version free-text "key:: value"
    # TRIAGE: class C — needs the block/typed-datatype join described in the
    # file header, plus a property-editing UI. Neither exists. This is the
    # scenario to revisit when block-generalization increment 2 (write
    # authority) lands: if blocks become typed entities, this turns into class B.
    When I add a property to a node
    Then I choose the property and a value of its declared type (text, number, date, node ref, checkbox, ...)
    And the value is stored as a typed value, not raw text

  @documented-only
  Scenario: Properties can be surfaced as query/table columns
    # TRIAGE: class B, and the closest of the four to reachable. A PRQL query
    # can already project any property via `json_extract`, so the data half
    # works today. Missing: a table view that renders those columns
    # sortable/filterable, and a Then over table columns. A sibling lane is
    # building the table widget — check with it before starting.
    Given nodes carrying a property
    When a query view includes that property
    Then the property appears as a sortable/filterable column

  @hover-revealed @observed
  Scenario: Hidden properties expand into typed key/value rows   # log:H10
    # TRIAGE: class C. Holon's drawer is a LAYOUT device — a query result row
    # with `collapse_to = "drawer"` (holon-api/src/render_eval.rs:374-438),
    # toggled by the ToggleDrawer transition — not a per-node property
    # inspector, and there is no hidden/system-property distinction to reveal.
    # Org `:PROPERTIES:` drawers are a disk format, not a rendered surface.
    When I click "Show hidden properties" on a node
    Then hidden props expand as typed rows (key with type icon, value, "---" when empty)
    And hovering a row reveals a leading bullet; the value is click-to-edit
