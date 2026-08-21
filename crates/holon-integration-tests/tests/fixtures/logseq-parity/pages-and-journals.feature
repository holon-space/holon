@core @observed
Feature: Pages and journals
  # Structurally observed across the session (shots 00-27) plus feature-inventory.

  # Journal day-page creation is BUILT and drivable in principle: `I advance the
  # clock by N days` drives the production `reconcile_clock`, whose CDC re-fires
  # the journal rule, and the wide seed's injected clock sits at a fixed
  # 2026-01-15 so a one-day advance lands on a KNOWN date rather than wall-clock
  # "today" — i.e. `Then within 15 seconds the widget contains "2026-01-16"` is a
  # well-formed assertion.
  #
  # It cannot run yet for a HARNESS reason, not a product one: `SutClockAdvance`
  # is deliberately env-gated (composed/builder.rs registers it only under
  # HOLON_PBT_ADVANCE_DAY — see WIDE_HEADLESS_ABSENT_CAPS in
  # composed/wide_e2e.rs:762), so `AdvanceDay` is outside the composed
  # alphabet and the replay rejects it on preconditions. `NamedFixture` already
  # carries an `env_flags` field; a `.feature` has no way to set it. Un-`@wip`
  # this once the flag is on by default, or once the step vocabulary can
  # declare a fixture's env flags.
  #
  # REVERSE-CHRONOLOGICAL ORDER is separately unstatable: `the widget contains`
  # is a substring test over a flattened snapshot, so it cannot say
  # "2026-01-16 comes before 2026-01-15". That needs an ordered-sequence
  # assertion.
  @wip @observed
  Scenario: The default view is a reverse-chronological journal feed
    Then the Journals view shows the current day page (e.g. "2026-08-20") at top
    And earlier day pages ("2026-08-19") follow below
    And each day page is an independent page identified by its date

  # Holon HAS the mechanism — `block.create_page_from_link` mints the page chain
  # for a dangling `[[Target]]` and heals the junction
  # (crates/holon/tests/create_page_from_link.rs). It is not reachable from a
  # fixture: the only step bound to it, `I create a page at path {path}`, gates
  # on the reference's FREED-path ledger, so it can fire only after a
  # `RenamePage` vacated that exact name
  # (transitions/create_page_at_freed_path.rs:60-86). Needs a transition for the
  # first-creation case, or a click-the-inline-link step.
  @wip @observed
  Scenario: Referencing a non-existent page creates it lazily
    # log:11 — same mechanism as page references
    When I reference "[[Project Alpha]]" and no such page exists
    Then the page is created on demand and appears in Recent

  # No deadline/scheduled -> date-page machinery exists: DEADLINE is carried by
  # the generic property mechanism only, with nothing that mints or links a date
  # page from it.
  @wip @observed
  Scenario: A referenced date creates a date/journal page
    # log:17/26 — the deadline date 2026-08-22 appeared as a page node
    When a Deadline "2026-08-22" is set
    Then a date page "2026-08-22" exists and is searchable

  # Properties exist and are typed, but no assertion can read a property VALUE
  # off a block or a column off a view — the assert vocabulary reaches rendered
  # text, focus, and parentage only.
  @wip @documented-only
  Scenario: Page properties
    # From feature-inventory; DB version stores properties as first-class typed nodes.
    When I add properties to a page
    Then they are stored as typed property values on the page node
    And they can be queried and shown as columns in views

  # Hover reveal is a frontend concern with no SUT transition (`on_hover` is a
  # positional-children widget builder), and the headless slice has no pointer.
  # Belongs to a windowed GPUI PBT.
  @wip @hover-revealed @observed
  Scenario: Heading hover exposes icon and property controls   # log:H1, H5
    When I hover a journal date heading or a page title
    Then "Add icon" and "Set property" appear above it
    And a journal heading also shows its "#Journal" tag on the right
