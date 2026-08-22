@core @observed
Feature: References and backlinks
  # Expectations distilled from interaction-log entries 11-13.

  @wip @observed
  Scenario: [[ opens page-reference autocomplete, auto-closes brackets, creates on the fly   # log:11
    When I type "Link to [[Project Alpha" in a block
    Then the closing "]]" is auto-inserted
    And an autocomplete popup appears
    And with no existing match it offers "New page Project Alpha"
    When I accept "New page Project Alpha"
    Then a page node is created with name "project alpha" (normalized) and title "Project Alpha"
    And the authoring block stores a reference to that node (not literal text)

  # Un-`@wip`ed 2026-08-22 (lane gap-refs) — the first half of log:12: a page
  # reference is stored AS A REFERENCE, not as literal text.
  #
  # Born-booted onto the wide seed, so no `an org file` / `the app is started`
  # ceremony: the files are written post-boot and ingested by the file watcher.
  # The rendered block shows the LABEL with the `[[ ]]` delimiters stripped,
  # which is only true if the parser turned them into a Link mark that survived
  # into the store and back out through the renderer.
  #
  # This is the FIRST parity scenario that seeds inline markup from an org file.
  # It was structurally impossible until the reference leg stopped discarding
  # the marks the production parser had already extracted — see bugfunnel
  # 2026-08-22-ref-seed-org-file-drops-inline-marks.
  #
  # SCOPE — what this pins. It pins the MARK ROUND-TRIP (parser -> reference
  # leg -> store -> renderer) AND, since 2026-08-22 (lane gv-vocab), reference
  # RESOLUTION: the last `Then` reads `block_links.resolved_id` directly, so a
  # DANGLING `[[Project Alpha]]` — which renders identically and used to pass
  # here — now reds. That makes the `Project Alpha.org` file below load-bearing
  # rather than decorative: it is the block the link must resolve TO.
  #
  # `block:ref-doc-N` is the ORACLE's synthetic document id, minted one per
  # `an org file` step IN ORDER (reference_state.rs). So `ref-doc-0` is
  # `Project Alpha.org` and `ref-doc-1` is `2026-08-20.org` — the day page,
  # which is what the focus step below opens. Reorder the two `Given`s and the
  # ids swap.
  @observed
  Scenario: A page reference is stored as a reference, not literal text   # log:12
    Given an org file "Project Alpha.org":
      """
      * Scope
      :PROPERTIES:
      :ID: alpha-scope
      :END:
      """
    And an org file "2026-08-20.org":
      """
      * Link to [[Project Alpha]]
      :PROPERTIES:
      :ID: referencing-block
      :END:
      """
    When I focus block "block:ref-doc-1" in region "main"
    Then within 15 seconds block "block:referencing-block" contains "Link to Project Alpha"
    # A `[[Page]]` reference resolves BY NAME to the PAGE — `ref-doc-0`, the
    # document `Project Alpha.org` mints — not to the `alpha-scope` headline
    # inside it. That is what makes `Project Alpha.org` load-bearing: delete it
    # and this link dangles.
    And within 15 seconds block "block:referencing-block" resolves link "Project Alpha" to block "block:ref-doc-0"

  # log:12's second half. `I click block` clicks a BLOCK; there is no step for
  # clicking an inline link inside one, and no Recent list exists.
  @wip @observed
  Scenario: Clicking a page reference navigates to the page   # log:12
    When I click the "Project Alpha" link inside the referencing block
    Then the Project Alpha page opens
    And it is added to the Recent list

  # log:13. Production backlinks are CORRECT end-to-end, verified 2026-08-22
  # against the real ingest path (TestEnvironment, SqlOnly): a bare
  # `[[Project Alpha]]` resolves BY NAME, `block_links.resolved_id` is set, and
  # the `backlinks` matview carries the referencing row.
  #
  # What blocks this scenario is OBSERVABILITY of the seeded main-panel
  # accordion from the composed headless slice. Two facts, both measured:
  #   * focusing the target page renders its outline and then nothing — the
  #     backlink row's text never reaches the widget snapshot;
  #   * `view_model_to_snapshot` (pbt/vm_snapshot.rs) copies props only for a
  #     fixed set of ViewKinds, so a generic widget's props (the accordion's
  #     "Linked references" title) and a ViewKind::Error message reach no
  #     snapshot at all.
  # The composed catalog's `inv-viewmodel-no-error-widgets` would catch an error
  # widget independently, so the open question is why the accordion's
  # `live_query` yields no row here when the same query yields one in prod —
  # most likely the `focus_roots`/`navigation_cursor` join this harness leaves
  # unsatisfied. Resolve that before un-`@wip`ing.
  # RE-MEASURED 2026-08-22 (lane gv-vocab) with the steps below run for real.
  # The OBSERVABILITY half of the blocker is CLOSED: `view_model_to_snapshot`
  # (pbt/vm_snapshot.rs) now copies props for EVERY `ViewKind` — the match is
  # exhaustive, so a generic widget's title and a `ViewKind::Error` message
  # both reach the snapshot. The widget dump for this scenario now carries
  # icons, drop-zone op names, tree-item depths and the view-mode switcher's
  # modes, none of which were visible before.
  #
  # What that measurement then proves: the section is ABSENT, not invisible,
  # and not degraded to an error widget — the dump contains no "Linked
  # references" title and no error message anywhere. So the ONLY remaining
  # blocker is the accordion's `live_query` yielding no row in this harness
  # (piece 2 of the bugfunnel entry): its join of `backlinks` to `focus_roots`
  # and `navigation_cursor` for region `main` (assets/default/index.org:23) is
  # unsatisfied here, while the same query returns a row against the real
  # ingest path. Un-`@wip` once that join is localized and fixed; the `Then`s
  # below are ready and need no further vocabulary.
  @wip @observed
  Scenario: Linked references list the backlinks grouped by source   # log:13
    Given an org file "Alpha.org":
      """
      * Project Alpha
      :PROPERTIES:
      :ID: alpha-page
      :END:
      """
    And an org file "Day.org":
      """
      * Link to [[Project Alpha]]
      :PROPERTIES:
      :ID: linking-block
      :END:
      """
    When I focus block "block:ref-doc-0" in region "main"
    Then within 15 seconds the widget contains "Linked references"
    And within 15 seconds the widget contains "Link to Project Alpha"

  @wip @observed @power
  Scenario: The (( )) block-ref syntax is removed; use [[ ]] for all node refs   # log:19
    # NOTE: candidate deliberate-deviation (file version uses ((uuid)) for block refs)
    When I type "((" to reference a block
    Then a toast appears: "To reference a node, please use `[[]]`."
    And no block-reference autocomplete is shown

  @wip @observed @power
  Scenario: [[ ]] references any node — pages and blocks alike   # log:20, log:21
    When I type "[[This is today" where a block with that content exists
    Then the autocomplete offers that existing block (grouped under its page) and a new-page option
    When I select the existing block
    Then the reference renders as a clickable node link
    And the referenced block shows a numeric reference-count badge
    And clicking the reference navigates/zooms to the target block

  @wip @observed @power
  Scenario: Node embed transcludes a node inline   # log:22
    When I run "/node embed" and pick an existing node
    Then a block renders the target's content transcluded, prefixed with a "→" embed indicator
    And the embed updates live with the source node

  @wip @observed
  Scenario: Unlinked references list plain-text mentions   # log:23
    Given the page title appears as plain text (no [[ ]]) in another block
    When I open the page and expand the "Unlinked references" section (collapsed by default)
    Then the plain-text occurrences are listed grouped by source page with the title highlighted
    And each can be converted into a real link
