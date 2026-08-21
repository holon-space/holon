@wip @core @observed
Feature: Search, command palette, and sidebar navigation
  # Expectations distilled from interaction-log entries 17-18 plus feature-inventory.
  #
  # Triaged 2026-08-22 (lane gap-refs). This whole feature is class C — the
  # functionality does not exist yet — so it stays `@wip` at the feature level
  # and no step here is expressible. Per-scenario evidence is recorded below so
  # the next lane does not have to re-derive it.

  # NOT BUILT. There is no search UI, no palette, no fuzzy matcher, and no
  # binding: assets/default/keybindings.yaml carries only leader-chord
  # navigation (Space+key), with no Cmd+K. Searched: palette, cmd_k, quick_open,
  # fuzzy, search across holon-app / holon-frontend / holon-gpui / keybindings.
  # Largest single item in this cluster — a palette needs a query surface, a
  # ranked multi-entity index (nodes, commands, files), and a create-on-miss
  # action.
  @observed
  Scenario: Cmd+K opens a unified search + command palette   # log:17
    When I press Cmd+K and type "Project"
    Then a palette shows a "Create page called 'Project'" action
    And matching Nodes (pages and blocks) with the query highlighted
    And a Recently-updated list
    And scope Filters: Search only nodes / codes / commands / files / themes
    # Commands are searched from the same palette (no separate command palette)

  # NOT BUILT — depends entirely on the palette above. Note that two of the
  # three actions DO exist as operations underneath: `navigation_open_tab` is
  # already bound to cmd/ctrl-click in the sidebar item template
  # (assets/default/index.org), so "open in sidebar" is a binding question, not
  # a new capability. "Copy ref" has no clipboard operation.
  @observed
  Scenario: Search results expose open / open-in-sidebar / copy-ref actions   # log:18
    Given the Cmd+K palette has a result selected
    Then the actions available are Open (Enter), Open in sidebar (Shift+Enter), Copy ref (Cmd+C)

  # PARTIAL. The left sidebar exists and renders the Page list plus an
  # Integrations section (assets/default/index.org, the `left_sidebar::render::0`
  # source). Journals is present as an ordinary page. MISSING: Favorites, a
  # Recent list, a Flashcards entry, a Graph view, and the graph switcher.
  # "Recent" has no backing concept — `navigation_history` is a back/forward
  # stack, not a recency-ranked list — so it is a data-model addition, not just
  # a widget.
  @documented-only
  Scenario: Left sidebar navigation
    # Observed structurally in every screenshot; not individually driven.
    Then the left sidebar shows Journals, Flashcards, Pages, Graph view
    And Favorites and Recent sections
    And the current graph name with a graph switcher at the top

  # INFRASTRUCTURE ONLY. `Region::RightSidebar` exists (holon-api/src/types.rs)
  # and the default layout seeds a `default-right-sidebar` container, but there
  # is no stacking, no reordering, and no shift-click binding that opens a
  # reference into it. Also blocked for testing by the same limitation the
  # references cluster hit: a click on an INLINE link has no step vocabulary.
  @documented-only
  Scenario: Right sidebar holds multiple context panels
    # From feature-inventory; opened via Shift+Enter / Shift-click.
    When I Shift-click a page or block reference
    Then it opens as a panel in the right sidebar without leaving the current page
    And multiple panels can be stacked and reordered

  # PERCEPTION / windowed. `on_hover` exists as a widget builder (first child is
  # the always-visible trigger, the rest reveal on hover) but hover is a
  # frontend concern with no SUT transition, and the headless slice has no
  # pointer. Belongs in a windowed GPUI PBT, not this corpus.
  @hover-revealed @observed
  Scenario: Sidebar row hover exposes a context menu   # log:H4
    When I hover a row in the left sidebar (Recent/Favorites/page)
    Then a "⋯" more-actions menu button appears at the right of the row
