@power @observed
Feature: Queries (DB version — visual filter builder + table views)
  # Expectations distilled from interaction-log entries 14-16.
  # DB-version simple queries are a structured filter builder rendering a live
  # table VIEW, not a {{query}} text DSL. Advanced (datalog) queries remain available.
  #
  # Scenario 2 was RULED a deliberate deviation on 2026-08-26 (D22.b) and is
  # rewritten below as its inverse. The rest stay @wip.

  @wip @observed
  Scenario: /query offers three query kinds   # log:14
    When I type "/query"
    Then the menu lists "Query", "Query function", and "Advanced Query"

  # RULED 2026-08-26 by Martin (D22.b). LogSeq DB composes a simple query with
  # a VISUAL FILTER BUILDER: a `#Query`-tagged block, a "+ Filter" control, a
  # fixed dimension palette (Tags / Page reference / Property / Task / …) and
  # and/or/not operator chips. Holon deliberately keeps queries AUTHORED AS
  # TEXT — the query is source, in a named query language, living in the
  # document. There is no filter builder and none is planned; `grep -r filter
  # frontends/gpui/src` finds no builder UI.
  #
  # This is not a poor-man's substitute for the builder: it is the same
  # authoring surface production itself uses. `assets/default/index.org` drives
  # the real UI's panels this way — a `#+BEGIN_SRC holon_sql` / `holon_gql`
  # source block for the data (`index.org:14`, `:25`, `:41`) and a
  # `#+BEGIN_SRC render` block for the presentation. Query text is the artifact
  # either way: the render blocks themselves carry inline
  # `live_query(#{sql: "…"})` calls (`index.org:12`, `:23`). Both forms
  # round-trip to disk, diff, and are edited as text.
  #
  # What this pins, and what it deliberately does not. The gate below proves
  # the AUTHORED TEXT is what drives the block: the SQL a user wrote in the
  # document runs, and its rows land inside that block's subtree. The block
  # additionally carries a "source" VIEW MODE next to its result layouts
  # (measured: its view-mode switcher offers table_view / board_view /
  # tree_view / source), which is where the text is edited — but the mode
  # switch is a `vms_button` geometry click and is NOT drivable headless
  # (`click_at_element` resolves the handle to an entity click, which no
  # headless widget answers). Asserting the SQL text on the surface therefore
  # waits for a windowed (GPUI) arm; the authored-text-drives-the-block half is
  # gated here.
  @observed
  Scenario: A query is composed as source text in a named query language
    Given an org file "Report.org":
      """
      * Blocks the query found
      :PROPERTIES:
      :ID: query-host
      :END:
      #+BEGIN_SRC holon_sql :id query-host::src::0
      SELECT b.* FROM block b WHERE b.content = 'alpha needle'
      #+END_SRC
      * alpha needle
      :PROPERTIES:
      :ID: needle
      :END:
      """
    When I focus block "block:ref-doc-0" in region "main"
    # The text-authored query RAN. `needle` is a SIBLING of `query-host`, not a
    # child of it, so the only way its content can appear INSIDE the query
    # block's subtree is as a result row the authored SQL produced. Change the
    # SQL's literal and this reds.
    #
    # The needle is kept inside the SAME document here so this scenario pins
    # only the authored-text-drives-the-block claim. The cross-document case —
    # a query legitimately drawing a row from OUTSIDE the focused subtree — is
    # its own scenario below.
    Then within 15 seconds block "block:query-host" contains "alpha needle"
    # …and it ran SUCCESSFULLY. Without this the assertion above is satisfiable
    # by a rendered FAILURE: `ui_watcher` turns a broken query into an error
    # widget whose message quotes the SQL that failed, and that message
    # contains the needle's text. Verified 2026-08-26 — appending `AND 1=0` to
    # the SQL above made the whole scenario pass on the error message alone.
    #
    # This assertion is also the only thing watching for such an error here:
    # `inv-viewmodel-no-error-widgets` walks from `root_layout_block_uri()`
    # only, so an error widget inside a per-block live tree is outside its
    # reach, and the render failure itself is merely a `tracing::warn!`.
    And block "block:query-host" renders no error widget
    # None of the DB-version builder chrome exists: no "+ Filter" control, no
    # dimension palette, no and/or/not operator chips, and no #Query class.
    And the widget does not contain "+ Filter"
    And the widget does not contain "#Query"
    And the widget does not contain "Page reference"

  # RULED 2026-08-26 by Martin (D36.a). A query in a page body may surface rows
  # from ANYWHERE in the graph — that is what a query is for, and the whole
  # point of authoring one in a document rather than typing an outline. Until
  # this ruling `inv-main-panel-rows-match-focus` reported such a row as a stale
  # leftover of a previous navigation, so every fixture that wanted a query had
  # to keep its needle inside the focused document. This scenario is the
  # standing gate that the workaround is gone: the needle lives in a DIFFERENT
  # document, which the main panel does not render, and the query still draws it
  # into the focused page's query block.
  #
  # The invariant now stops its descent at a query surface instead of judging
  # its result rows as outline content. A row that reaches the panel by any
  # OTHER path is still flagged — see the counter-cases in
  # `pbt/invariants/bodies/main_panel_rows_match_focus.rs`.
  @observed
  Scenario: A query surfaces a row from outside the focused subtree
    Given an org file "Report.org":
      """
      * Blocks the query found
      :PROPERTIES:
      :ID: cross-doc-host
      :END:
      #+BEGIN_SRC holon_sql :id cross-doc-host::src::0
      SELECT b.* FROM block b WHERE b.content = 'beta needle'
      #+END_SRC
      """
    Given an org file "Elsewhere.org":
      """
      * beta needle
      :PROPERTIES:
      :ID: beta-needle
      :END:
      """
    When I focus block "block:ref-doc-0" in region "main"
    # `beta-needle` is not a descendant of the focused document at all, so no
    # outline path can put its content inside `cross-doc-host`. Only the
    # authored query can — and the invariant must accept it.
    Then within 15 seconds block "block:cross-doc-host" contains "beta needle"
    # Same guard as the scenario above: a rendered FAILURE quotes the SQL, and
    # that message contains the needle text.
    And block "block:cross-doc-host" renders no error widget

  @wip @observed
  Scenario: Query results render as a live, configurable table view   # log:16
    Given a #Query block
    When I add a Task filter and select the status "Done" and Apply
    Then the filter chip reads "task: Done"
    And a "Live query" result renders as a table with columns Name, Tags, Status, Deadline
    And the table has a toolbar for sort, filter, search, and view-layout switching
    And only nodes matching the filter appear as rows

  @wip @documented-only
  Scenario: Advanced (datalog) query
    # From feature-inventory; "Advanced Query" accepts a datalog/datascript query
    # with :query, :inputs, :result-transform, and rendering options.
    When I insert an Advanced Query with a datalog expression
    Then the raw datascript query is executed against the graph
    And the result set is rendered per the view/result options

  @wip @hover-revealed @observed
  Scenario: Query-row hover exposes open controls   # log:H6
    When I hover a row in a Live query table
    Then a "→" open-node button and a "▭" open-in-sidebar button appear in that row

  @wip @hover-revealed @observed
  Scenario: Query column headers are configurable   # log:H8
    When I click a Live query column header
    Then a column-config menu opens (sort, pin, property name/type, available choices, checkbox mapping, UI position, hide)
