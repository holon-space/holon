@wip @hover-revealed @observed
Feature: Hover-revealed affordances (cross-cutting)
  # In LogSeq (DB version) many controls are hidden until the pointer hovers the
  # element. Distilled from interaction-log entries H1-H7. Holon should expect the
  # same "hover to reveal, click to act" pattern on headings, titles, tags, rows.

  @core
  Scenario: Hovering a journal date heading reveals inline controls   # log:H1
    When I hover the journal date heading
    Then "Add icon" and "Set property" controls appear above the heading
    And the "#Journal" tag appears at the far right of the heading row
    And none of these are visible before hover

  @core
  Scenario: The same controls appear on any page title   # log:H5
    When I hover a normal page title
    Then "Add icon" and "Set property" appear above it
    # the heading-hover affordance is uniform across journals and pages

  @core
  Scenario: Hovering a block row reveals its handles   # log:H2
    When I hover a block row
    Then the bullet and a drag handle are shown
    And a parent block additionally shows its collapse/expand triangle

  @power
  Scenario: Hovering a tag reveals a remove control   # log:H3
    When I hover a "#Task" tag on a node
    Then the tag shows an inline "✕" to remove/unassign the class from the node

  @core
  Scenario: Hovering a left-sidebar row reveals a context menu   # log:H4
    When I hover a Recent/Favorites/page row in the left sidebar
    Then a "⋯" more-actions button appears at the right of the row

  @power
  Scenario: Hovering a query-result row reveals open controls   # log:H6
    When I hover a row in a Live query table
    Then a "→" open-node button and a "▭" open-in-sidebar button appear in the row

  @observed
  Scenario: A hover-revealed control opens a real flow   # log:H7
    When I hover a heading and click the revealed "Set property"
    Then an "Add or change property" typed-property picker opens
    And choosing a property adds it to the page

  @power
  Scenario: Query column header exposes a column-config menu   # log:H8
    When I hover a column header in a Live query table
    Then the header highlights as interactive
    When I click it
    Then a menu offers Sort ascending/descending, Pin, Property name, Property type,
      Available choices, Checkbox state mapping, UI position, Hide by default, Hide empty value

  @core
  Scenario: A plain block's right gutter has no controls   # log:H9
    # NOTE: candidate deliberate-deviation — block controls live in the LEFT gutter
    When I hover the right end of a plain block row
    Then no action controls appear on the right (only a node's tags render there when present)

  @power
  Scenario: Hovering a typed property row shows it is node-like   # log:H10
    When I expand hidden properties and hover a property row
    Then a leading bullet appears (rows are node-like) and the empty value "---" is click-to-edit

  @core
  Scenario: Linked-references section header exposes a toolbar   # log:H11
    When I hover the "Linked references" section header
    Then a collapse triangle, an add "+", and a right-side toolbar (filter, sort, filter, search, layout, more) appear
