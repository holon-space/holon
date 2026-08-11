# Plan — LogSeq-style lazy Journals view + hidden-by-default `holon_rule` blocks

Planning lane, 2026-08-11. No implementation in this lane.
Base sentinel: `grep -q "stripe_of" crates/holon/src/api/operation_engine.rs` → **PASS**.

> **Headline for Martin, before anything else.** Most of the feature you
> described is *already designed and already implemented* — one view, one date
> heading per day, newest first, day content inline, `divider()` between days.
> It is unreachable, not unbuilt: the app's own focus-navigation path does not
> show the feed (an `#[ignore]`d, red-on-main test says so in as many words).
> And SQL-side `LIMIT` windowing — the obvious way to make it lazy — is
> **deliberately forbidden** by a contract already written into the code. So
> this plan is mostly "make the existing thing reachable and viewport-driven",
> not "build a new view".
>
> **All four §8 questions are now RULED by Martin (2026-08-11), each per
> recommendation.** §8 records them; the sections above are written to match.

---

## 1. FACTS FIRST

Every fact below is `file:line` on this base. §7 lists the greps to re-run at
the start of each increment; a changed answer means the plan is stale.

### 1.1 How the Journals page renders TODAY

The Journals page is **three seeded blocks**, defined twice (programmatic seed
and on-disk asset) and required to agree:

- `crates/holon-frontend/src/lib.rs:101-140` — `journals_page_blocks()`:
  1. the `block:journals` page shell (`Block::new_text`, `set_page(true)`),
  2. a `holon_sql` source: **`SELECT * FROM journal_feed ORDER BY content DESC`**
     (`lib.rs:122`),
  3. a `render` source: **`list(#{sortkey: "-content", item_template: column(render_entity(), divider())})`**
     (`lib.rs:133`).
- `assets/default/Journals.org` — the byte-equivalent on-disk form, plus a
  `** Journal Auto-Create` heading owning the `daily_journal` `holon_rule`
  block (`:id journals::action::0`).
- `crates/holon-frontend/src/lib.rs:985-990` — a test pins that the feed
  surface reads `FROM journal_feed`.

So the target UX in Martin's spec is **the documented existing design**:
`docs/Plans/JournalFeed-2026-07-18.md:14-30` states it as "Logseq-style: a
scrollable reverse-chronological list of day pages, each inline-expanded".

**Date headings already exist as a row concept** — contrary to what a
generic "main-panel rows are flat" reading suggests. The heading is the
`header:` slot of the per-row `expand_toggle`:
`assets/default/types/block_profile.yaml:84-100`, variant
`embedded_page_expanded`:

```yaml
- name: embedded_page_expanded
  priority: 2
  condition: 'is_def_var("expand_default") && expand_default == 1 && (!is_def_var("role") || role != "page_title")'
  render: >-
    expand_toggle(#{
      default_expanded: true,
      hover_reveal_toggle: true,
      header: selectable(row(icon("orgmode", …), text(col("content"))), #{action: navigation_focus(…)}),
      content: live_query(#{prql: "from descendants", item_template: tree(…render_entity()…)})
    })
```

`text(col("content"))` **is** the date heading (day-page `content` is the
`YYYY-MM-DD` date), and it is a click-to-navigate `selectable`.

GPUI render path:

- `frontends/gpui/src/render/builders/live_query.rs:85-189` — lazily creates a
  `ReactiveShell` entity per live query.
- `frontends/gpui/src/views/reactive_shell.rs:924` — collections render through
  **`gpui::list()`**, i.e. painting is *already virtualized*.
- `frontends/gpui/src/views/reactive_shell.rs:1017-1048` — `render_row()`
  builds one cached `RenderEntityView` per row, keyed `CacheKey::RenderEntity`.
- `frontends/gpui/src/views/reactive_shell.rs:667-698` — `compute_visible_indices()`,
  an existing **display-level row filter** (today: tree-collapse only).
- `frontends/gpui/src/render/builders/mod.rs:267-321` — the widget dispatcher
  (`builder_registry!`), i.e. where a node's widget name picks a builder.

### 1.2 Does cursor / windowed pagination exist? **No — and it is refused by contract.**

`cursor_filtered_main_panel_delivers_at_vault_scale`
(`crates/holon/tests/turso_storage_repros/tabs_main_panel_delivery.rs:63-206`)
is **not** pagination. Its "cursor" is the **navigation cursor** — which tab is
active — joined in at `tabs_main_panel_delivery.rs:59`
(`JOIN navigation_cursor nc ON nc.region = fr.region AND nc.history_id = fr.history_id`).
It asserts a ~70-page vault's active-tab subtree materializes in <5s and
contains no other tab's rows. It reads with **no `LIMIT`**
(`tabs_main_panel_delivery.rs:172-175`). It proves tab isolation at scale; it
proves nothing about incremental fetch.

The load-bearing negative fact — **windowing is designed out of the read path**:

- `crates/holon-turso/src/util.rs:36-41` — `strip_order_by()` removes the
  trailing clause before matview DDL.
- `crates/holon-turso/src/util.rs:420-425` (test `strip_order_by_also_strips_limit`)
  — it strips `LIMIT` too.
- `crates/holon-turso/src/util.rs:43-48` — **the reason, in the source**:
  > "`LIMIT` / `OFFSET` are excluded: the matview holds the unbounded relation
  > and its CDC stream delivers changes beyond any window, so re-applying a
  > window to the snapshot alone would disagree with the stream."

So a `LIMIT` written into the journals `holon_sql` is silently discarded, *by
design*, because a windowed snapshot and an unbounded CDC stream cannot be
reconciled under the current delivery contract. Ordering is likewise not done
in SQL: it is lifted off (`backend_engine.rs:316-330`, `trailing_order_by` →
`order_by_sort_spec`) and re-applied by the renderer (`sortkey: "-content"`).

Consistent with this, the query API has no window parameters: `execute_query`
(`crates/holon/src/api/backend_engine.rs:492`) and `query_and_watch`
(`:576`, CDC subscription at `:607-608`) take params but no
`limit`/`offset`/`after`; `QueryContext` (`crates/holon/src/api/query_context.rs:40-91`)
carries only `current_block_id`, `context_parent_id`, `path_context`.

`docs/Plans/JournalFeed-2026-07-18.md:80-81` and
`crates/holon-turso/sql/schema/journal_feed_matview.sql:8-9` both call the
`journal_feed` matview "the seam where feed windowing / LIMIT will live
(increment 2)". **That deferred plan is in direct tension with `util.rs:43-48`.**
RULED (§8.1) in favour of `util.rs`: view-layer windowing, `journal_feed` stays
unbounded, and the deferred-LIMIT note is amended in increment 0.

### 1.3 How date pages are keyed and ordered

- Detection: `crates/holon-turso/sql/schema/journal_day_pages_matview.sql:36-39`
  — `FROM block b JOIN block_tags bt ON bt.block_id = b.id AND bt.tag = 'Page' WHERE b.parent_id = 'block:journals'`.
- Key: the day page's `content` is the date, `YYYY-MM-DD`
  (`journal_day_pages_matview.sql:3`). Lexicographic order == chronological order
  — this is why `content DESC` is a correct newest-first key and why a *keyset*
  cursor would be trivial if the delivery model allowed one.
- Feed projection: `crates/holon-turso/sql/schema/journal_feed_matview.sql:11-34`
  adds `1 AS expand_default` and nothing else.
- Creation: the `daily_journal` rule (`assets/default/Journals.org`, mirrored at
  `crates/holon/src/api/holon_rule_watcher.rs:598-601`) emits
  `place: page(journals)`, `name: "{today}"` — so each day is its **own page
  file** `Journals/<date>.org`, `Page`-tagged.
- Ordering **is not** applied in SQL (stripped, §1.2); it is applied by the
  render `list(#{sortkey: "-content"})`.

### 1.4 Where the renderer decides what to draw — the display-filter hook

`assets/default/types/block_profile.yaml` is the canonical per-block render
decision point (`yaml:5-7`: "every block goes through `live_block` →
`render_entity` → TypeRegistry → block_profile"). It carries **computed fields**
and **priority-ordered variants**:

- `yaml:45` — `is_rule_head: 'is_source && (source_language == "holon_rule" || source_language == "action")'`
- `yaml:52` — `is_program: 'is_rule_head || (is_source && rule_sibling(parent_id) != ())'`
- `yaml:125-133` — variant `rule_card`, **priority 0**, `condition: 'is_program'`,
  renders a `card(...)` with "Automation rule", an Enabled checkbox and
  `text(col("content"))`. **This is what draws a `holon_rule` block today.**
- `yaml:134-136` — variant `source_editing`, **priority −1**,
  `condition: 'is_source && is_focused'` → `source_editor(...)`.
- `yaml:138-140` — variant `holon_source`, priority −1 → **`spacer(0)`**:
  precedent for *hiding a block at the display level while leaving storage
  untouched* (used for `holon_sql` / `render` machinery, `yaml:25`).
- `yaml:102-116` — variant `embedded_page`, priority 1: the **collapsed, lazy**
  `expand_toggle` whose `content:` `live_query` is only materialised on expand.

So there are two hiding precedents already: `spacer(0)` (hard hide) and
collapsed `expand_toggle` (hide-with-disclosure). The second is exactly the
affordance Martin asked for.

Note the precedence hazard, which the plan must handle: `rule_card` is
**priority 0** and `source_editing` is **priority −1**, so a *focused* rule
block gets the card, **not** the editor. Edit-in-place is blocked by this
ordering today (`yaml:125` vs `yaml:134`).

### 1.5 Do query-rendered blocks share the standard editor path? **Yes.**

- `frontends/gpui/src/render/builders/editable_text.rs:10-70` — `EditorView`
  entities are keyed by `row_id + field` via `LocalEntityScope::get_or_create()`
  (`:47`), identically for a home-document row and a query-result row.
- `frontends/gpui/src/views/render_entity_view.rs:22-75` — query-rendered rows
  own their own `entity_cache` (`:34`), so nested editors survive
  `VecDiff::UpdateAt`.
- `frontends/gpui/src/views/editor_view.rs:726-736` and `:1079-1087` — the same
  `grab_focus_and_seed_caret()` runs regardless of row origin.

**Consequence:** edit-in-place from the journals feed needs *no* new editor
plumbing. It needs only the variant-precedence fix in §1.4.

### 1.6 The blocker nobody has removed

`crates/holon-integration-tests/src/pbt/frontend_slice/structural_pbt.rs:4021-4023`:

```rust
#[ignore = "RED on main: journal feed render_source unreachable via focus \
            navigation — open architecture bug (BugFunnel row 34)"]
async fn journal_feed_via_main_panel_focus_shows_feed() {
```

The test renders the journals page **directly** (`widget_tree_for(&journals)`,
`:4072-4076`) and gets the correct feed, then navigates the **app path**
(`apply_navigate_focus(CapRegion::Main, &journals)`, `:4053`) and does not. Its
assertions are already exactly the spec: newest-first (`:4085-4088`),
every entry `expanded=true` (`:4090-4096`), one `divider()` per entry
(`:4097-4101`).

**This is a ready-made, red-for-the-right-reason rung for increment 1**, with
its red already documented on main. Until it is green, none of the rest of this
feature is visible to Martin in the running app.

---

## 2. TARGET ARCHITECTURE

### 2.1 First principles — what "lazy" has to mean here

The requirement is *"render cost does not scale linearly with history size"*.
Break the cost into its three independent terms:

| # | Cost term | Scales with | Status today |
|---|---|---|---|
| **a** | Feed **rows delivered** by `journal_feed` CDC | # day pages (~365/yr) | Unbounded, but each row is one thin block row |
| **b** | **Painting** those rows | # day pages | **Already solved** — `gpui::list()` virtualizes (`reactive_shell.rs:924`) |
| **c** | Per-day **content materialisation** | # day pages × day size | `expand_default = 1` forces a `live_query(from descendants)` for **every** day → one `ReactiveShell` + one watched matview + one CDC subscription **per day page** (`journal_feed_matview.sql:32`, `block_profile.yaml:84-100`, `live_query.rs:85-189`, `backend_engine.rs:576,607`) |

Term (c) is the dominant one and the one nobody has addressed: the feature that
makes the feed *look* like LogSeq (default-expanded days) is precisely what
makes it cost O(history). Term (a) is thin rows; term (b) is already free.

The instinct — "put a `LIMIT` in the feed matview" — attacks (a), the *cheap*
term, and is refused by `util.rs:43-48` anyway. **This plan attacks (c).**

### 2.2 The design: viewport-driven expansion, reusing the already-lazy path

The collapsed `embedded_page` variant (`block_profile.yaml:102-116`) is
**already lazy**: its `live_query` content is only materialised on expand, and
that laziness is already pinned by the `embedded_page_collapsed_lazy` invariant
(`crates/holon-integration-tests/src/pbt/invariants/bodies/embedded_page_collapsed_lazy.rs:60-120`)
and by `embedded_page_renders_collapsed_and_lazy`
(`structural_pbt.rs:3761-3800`).

The `embedded_page_expanded` variant defeats that laziness by hardcoding
`default_expanded: true` for *every* feed row.

**Target:** replace the *static* `default_expanded: true` with a
**viewport-driven expand gate**. A day row is expanded iff it is inside the
scroll viewport plus a small overscan margin; rows that leave the window
collapse and release their `live_query` shell. Result:

- expanded (i.e. materialised) days ≈ **O(viewport)**, constant in history;
- the visual result is byte-identical to today for anything the user can see —
  every day the user scrolls to is expanded, with its date heading, in one
  continuously-scrolling view;
- **no new query infrastructure**: the window is applied downstream of the
  unbounded CDC stream, which is exactly the discipline `util.rs:43-48`
  demands (order and window are the *reader's* job, not the matview's — the
  same convention that already puts `sortkey: "-content"` in the render spec).

Seams it composes, none of them new:

| Need | Existing mechanism |
|---|---|
| Which rows are on screen | `gpui::list()` + `BoundsRegistry` (`reactive_shell.rs:924`, `frontends/gpui/tests/main_panel_scroll.rs:66,74`) |
| Row-level display filtering | `compute_visible_indices()` (`reactive_shell.rs:667-698`) |
| Per-row expand state | `expand_toggle` gate over `UiState.expanded_view` (`structural_pbt.rs:3808-3856`) |
| Lazy content behind the gate | collapsed `embedded_page` `live_query` (`block_profile.yaml:102-116`) |
| Ordering | render `list(#{sortkey: "-content"})` (`holon-frontend/src/lib.rs:133`) |

### 2.3 Where the machinery does **not** fit — say it plainly

1. **SQL/keyset pagination is unavailable and should not be built for this.**
   `util.rs:43-48` states the contract; `strip_order_by` enforces it
   (`util.rs:36-41`, test `:420-425`). Building cursor pagination means adding a
   window parameter to `QueryContext`/`execute_query`/`query_and_watch` **and**
   solving snapshot-vs-stream reconciliation for a windowed relation — a
   substantial new delivery protocol touching every reactive surface, not just
   journals. Deferring `docs/Plans/JournalFeed-2026-07-18.md:81`'s "increment 2
   LIMIT in `journal_feed`" is not laziness; it is the correct reading of the
   contract that was written *after* that plan.

2. **Term (a) is left unbounded, deliberately.** With ~365 thin rows/year and
   `gpui::list()` virtualization, this is very likely a non-issue for years.
   **But it is an assumption, not a measurement** — which is why increment 0
   measures it before anything else, and why a "load older" affordance stays on
   the shelf as a fallback rather than being pre-built.

3. **The feed is not reachable from the app's own navigation at all** (§1.6).
   No amount of laziness matters until that is fixed, so it is increment 1.

### 2.4 Rule hiding — display layer only

`holon_rule` blocks are hidden by **changing which variant draws them**, in
`assets/default/types/block_profile.yaml`. Nothing touches org, ingest, the
watcher, or storage — the exact same lever `holon_source`/`spacer(0)`
(`yaml:138-140`) already pulls for query machinery.

- **Hidden by default + reveal:** `rule_card` (priority 0, `condition:
  'is_program'`) becomes a **collapsed `expand_toggle`** whose `header:` is a
  quiet disclosure affordance and whose `content:` is today's rule card. This
  reuses the `embedded_page` shape verbatim (`yaml:102-116`) — same widget, same
  gate, same lazy content — so it inherits the existing expand-toggle
  invariants instead of inventing a hide mechanism.
- **Revealed rule editable in place:** the blocker is precedence, not plumbing
  (§1.4, §1.5). `source_editing` (`yaml:134-136`, priority −1) must beat the
  rule variant when `is_focused`. The fix is a precedence change, and the
  `block_profile.yaml:118-124` comment explicitly warns that priority-0
  precedence here is *documented, not implied* — so it must be re-documented,
  not quietly flipped.
- **Scope: ALL of `is_program`** (`yaml:52`) — RULED (§8.2), not `is_rule_head`
  (`yaml:45`) alone. `is_program` also matches a rule's *trigger sibling*, so
  head and trigger are hidden together and a rule is never half-hidden. This is
  a visible behaviour change on **every** page hosting a rule, not only
  Journals — increment 3's rung asserts the scope on a non-journals page too.
- **The disclosure toggle ships atomically with the hiding** — RULED (§8.3).
  There is never a tree state in which rules are hidden with no affordance to
  reveal them, so increment 3 is not blocked on the (unruled) Rules page.

---

## 3. INCREMENT PLAN

Risk-elimination first. Each increment is independently landable and leaves the
tree strictly better. Each names its rung and its tier per the `holon-feature`
skill (red-for-the-right-reason **before** implementation; red log in the PR;
`dogfood-explorer` as the last gate).

### Increment 0 — MEASURE (no production change)

**Why first:** the whole design in §2.2 rests on the claim that term (c)
dominates and terms (a)/(b) do not. That claim is *reasoned from the code*, not
measured. If it is wrong, increments 2's design changes. Measure before
building.

**Do:** a vault-scale journals fixture (N ∈ {30, 365, 1095} day pages, each with
a handful of children — extend the existing seed at
`crates/holon-integration-tests/scripts/seed_wide/Journals.org`), and a
measurement harness that reports, per N: watched-matview count, CDC
subscription count, `ReactiveShell` count, and interaction→projection-visible
wall time.

**Rung (headless keystone):** `journals_feed_cost_is_sublinear_in_history` —
asserts materialised-`live_query` count is bounded by a constant, independent
of N.

**Expected red:** at N=365 the count equals N (one per day page) because
`expand_default = 1` (`journal_feed_matview.sql:32`).

**Honest caveat — this rung may not go red, and that is a result, not a
failure.** `gpui::list()` (`reactive_shell.rs:924`) may already avoid
constructing off-screen rows, in which case term (c) is *already* viewport-
bounded in the windowed frontend and only the headless path is eager. If the
rung cannot be made red for the intended reason, **stop and report** — do not
fake a red (`holon-feature` §1). The finding would redirect increment 2 from
"add viewport-driven expansion" to "fix the headless/windowed divergence", and
should be recorded via `bug-gap-triage` as a perception gap.

**Ships:** the fixture and the measurement rung — both permanently useful
regardless of the answer — **plus the §8.1 documentation duty**: amend
`docs/Plans/JournalFeed-2026-07-18.md:80-81` and its "increment 2 (deferred):
feed windowing/pagination (LIMIT in `journal_feed`…)" line at `:95` to record
that view-layer windowing is RULED and SQL-side `LIMIT` is refused, citing
`crates/holon-turso/src/util.rs:43-48`. Leaving that note unamended is exactly
how the next agent rebuilds the refused design (R9).

### Increment 1 — Make the feed reachable via the app's own navigation ⟵ **the blocker**

**Why:** §1.6. Direct render shows the feed; focus navigation does not. Nothing
Martin can see changes until this lands.

**TIMEBOXED, IN-LANE — RULED (§8.4).** Fix it here **only** if the repair stays
inside the journals/focus rendering path. **STOP, revert to a clean tree, and
report for escalation** the moment the smallest working fix would change
delivery or navigation architecture — `backend_engine.rs` query-delivery
signatures, the CDC delivery contract, `QueryContext`/`execute_query` shapes,
the region/tab navigation model, or how any surface *other than* the journals
feed resolves its render source. Absorbing an architecture change under this
lane is the failure mode this criterion exists to prevent.

**Rung (headless keystone):** remove `#[ignore]` from
`journal_feed_via_main_panel_focus_shows_feed`
(`structural_pbt.rs:4021-4024`). Its red is already documented on main and its
assertions already encode the spec (newest-first `:4085`, all-expanded `:4090`,
one divider per entry `:4097`).

**Also:** a windowed rung asserting the same three properties through the real
GPUI window, since "the user sees it" is the point
(`frontends/gpui/tests/main_panel_scroll.rs` is the template; harness entry
`TestAppContext::add_window_view()` + `ReactiveFixtureView`).

**Note:** the all-expanded assertion at `:4090-4096` will need relaxing to
"expanded within the viewport" once increment 2 lands. Flagged here so it is a
deliberate edit with a stated reason, not a silently weakened assertion.

### Increment 2 — Viewport-driven expansion (the lazy part)

Only start once increment 0 has said where the cost is.

**Do:** replace the static `default_expanded: true` (`block_profile.yaml:89`)
with a viewport-driven gate; expand rows in viewport + overscan, collapse and
release the rest. Reuses `compute_visible_indices()`
(`reactive_shell.rs:667-698`), `BoundsRegistry`, and the existing
`expand_toggle` gate.

**Rungs:**
- headless: increment 0's `journals_feed_cost_is_sublinear_in_history` turns
  **green** — the constant bound now holds at N=1095.
- **windowed (the load-bearing one):** `journals_scroll_window_expands_and_releases`
  — drive real wheel events via `simulate_wheel_at()`
  (`frontends/gpui/tests/support/mod.rs:791-805`, used at
  `main_panel_scroll.rs:46`); assert (i) a day scrolled INTO view has its
  children rendered, (ii) a day scrolled far OUT of view has released them,
  (iii) the date heading of every on-screen day is present and in newest-first
  order. Its red before implementation: (ii) fails because everything is
  eagerly expanded forever.

**Continuity guard:** an invariant that scrolling never produces a frame with a
*gap* — no day page between the first and last on-screen dates may be missing.
Lazy loading's characteristic failure is a hole in the middle of the scroll,
and a naive "is it expanded" assertion will not catch it.

### Increment 3 — `holon_rule` hidden by default + disclosure affordance

**Do:** in `assets/default/types/block_profile.yaml`, turn the `rule_card`
variant (`:125-133`) into a collapsed `expand_toggle` wrapping today's card,
scoped to **all of `is_program`** (§8.2). Re-state the priority-0 precedence
comment (`:118-124`) to match the new shape.

**Binding acceptance criterion (§8.3):** the hiding and the disclosure toggle
land in the **same** increment — no intermediate state where a rule is hidden
and unreachable. The collapsed affordance must be visibly discoverable (a
marker with real height, never `spacer(0)`).

**Rungs:**
- headless: `holon_rule_hidden_until_disclosed` — on a page hosting a rule, no
  rule card widget is in the tree by default; after toggling the disclosure, it
  is. Red before: the card is present unconditionally.
- headless (**inertness — the invariant Martin actually cares about**):
  `holon_rule_hiding_is_display_only` — org bytes, `block_raw`, and the rule's
  `RuleStatus` are **byte-identical** hidden vs revealed, and the rule still
  *fires* while hidden. This is a display change that must not become a
  functional one; the `display_placement_canonical_inert` invariant
  (`crates/holon-integration-tests/src/pbt/invariants/bodies/display_placement_canonical_inert.rs:57-90`)
  is the shape to copy.
- windowed: the disclosure affordance is clickable and reveals.

### Increment 4 — A revealed rule is editable in place

**Do:** fix the variant precedence so `source_editing` (`yaml:134-136`) wins
over the rule variant when `is_focused` (§1.4). No editor plumbing — §1.5
establishes that query-rendered rows already share the standard editor path.

**Rungs:**
- headless: `revealed_holon_rule_edits_in_place` — focus a revealed rule, type,
  and assert the edit reaches `block_raw` **and** the on-disk
  `Journals.org` `holon_rule` body. Red before: focusing yields the card, so no
  editable surface exists.
- windowed: a real click into a revealed rule produces a caret and accepts
  keystrokes.

**Watch for:** the rule is a *source* block, so this crosses the `source_text`
projection channel landed for #78/#93. Confirm which channel a rule body
commits through before writing the rung; a wrong-channel commit is exactly the
class of bug tasks #99/#100 were.

### Increment 5 — Rules query page ⟵ **SEVERABLE, DO NOT BUILD**

A page listing every rule in the vault. **The display ruling is still PENDING
with Martin** — only questions 1–4 were ruled (§8). Do not build; do not let
increments 3–4 assume it exists. Per §8.3, increment 3 does **not** wait for it:
the atomically-shipped disclosure toggle carries discoverability meanwhile.

### Explicitly OUT OF SCOPE

- **Embedding / transclusion** — out, per the task. `RowOrigin::DisplayPlaced`
  and `OccurrenceId` (`crates/holon-frontend/src/row_origin.rs:45-54`) exist as
  scaffolding for it; **do not build on them here**. Journal day rows are
  canonical rows, not occurrences.
- **SQL/keyset pagination, `LIMIT` in `journal_feed`, cursor parameters in
  `QueryContext`** — refused by §2.3(1). Supersedes
  `docs/Plans/JournalFeed-2026-07-18.md:81`.
- **A "load older" button** — unnecessary under §2.2 (continuous scroll, no
  paging boundary). Fallback only if increment 0 shows term (a) is the problem.
- Calendar / date-picker UI; per-day summary or child-count columns
  (`JournalFeed-2026-07-18.md:96-97`, increment 3 there).
- Changing `daily_journal`, the clock model, or day-page identity.
- Seed-vs-file authority for `block:journals` (`journals_seed_file_collision.rs`,
  BugFunnel row 25) — a separate product ruling.
- Editing rules in any surface other than in-place reveal.

---

## 4. THE SLO — p95 interaction→projection-visible < 200 ms, applied to scrolling

**What the SLO means for a scroll:** the interaction is one wheel/trackpad
event; "projection-visible" is the first frame in which every day-page row
inside the new viewport shows its date heading **and** its content. Under
§2.2 that frame requires materialising the `live_query` of each day newly
entering the window.

**How the design respects it:**

1. **Bounded work per scroll event.** Only days crossing the window boundary
   materialise — at most a handful per event, independent of history size.
   This is the whole point of attacking term (c): without it, the very first
   paint pays O(history) and the SLO is unreachable at any scroll speed.
2. **Overscan margin.** The window is viewport + margin, so a day is
   materialised *before* it is visible and the user meets an already-drawn row.
   The margin is the single tuning knob; increment 2 must report the value it
   picked and the measured p95 at that value.
3. **Never a blank frame.** If a day cannot be materialised inside the frame
   budget it renders its **date heading immediately** with a disclosed loading
   state underneath — degraded *visibly*, per the project's error philosophy.
   A blank or missing day is a failure, not a slow path.
4. **Release is off the critical path.** Collapsing rows that left the window
   must not run inside the scroll frame; it is deferred work.

**How the rung measures it:** extend the windowed rung
`journals_scroll_window_expands_and_releases` (increment 2) into a latency
assertion, following the pattern already used for scroll in
`frontends/gpui/tests/main_panel_scroll.rs`:

- seed N = 365 and N = 1095 day pages (increment 0's fixture);
- drive a **burst** of `simulate_wheel_at()` events
  (`support/mod.rs:791-805`) — not one event, since p95 only means something
  over a distribution;
- per event, timestamp from dispatch to the first frame in which the new
  viewport's `BoundsRegistry` entries (`main_panel_scroll.rs:66,74`) cover
  every expected day id **with non-empty content**;
- assert **p95 < 200 ms** and, separately, that p95 does **not grow** from
  N=365 to N=1095 — the constant-in-history property is the real claim, and a
  fixed threshold alone would pass a design that is merely fast today;
- a latency regression here is a BugFunnel-reportable bug in its own right per
  the project rule ("latency above the SLO counts as such a bug").

**Honesty note:** headless timings do not model the GPUI paint path — the
arm-d report measured 236→243 ms headless for a change that was 16× faster
live. The latency claim must therefore be made by the **windowed** rung. A
headless number is a smoke check, never the SLO evidence.

---

## 5. RISK REGISTER

| # | Risk | Likelihood | Impact | Falsify / mitigate |
|---|---|---|---|---|
| R1 | **Increment 1's navigation bug is architectural** and larger than this feature (the `#[ignore]` calls it "open architecture bug") | High | Blocks everything | RULED in-lane but TIMEBOXED (§8.4). First thing after increment 0. STOP + revert + escalate if the fix would touch delivery/navigation architecture rather than the journals focus-render seam |
| R2 | **Increment 0's rung cannot go red** — term (c) is already viewport-bounded in the windowed frontend | Medium | Redesigns increment 2 | This *is* increment 0's job. Report, don't fake a red; re-triage via `bug-gap-triage` as a headless/windowed perception gap |
| R3 | **Collapse-on-release destroys editor state** — a day scrolled out of view while being edited loses the caret, or worse, a pending commit (cf. the #94/#99 commit-funnel bugs) | Medium | **Data loss** — the worst outcome here | Never release a row containing focus. Make it an invariant of increment 2, not a code comment. Pin it with a rung: edit a day, scroll it far away, scroll back, assert text and caret survived |
| R4 | **`is_program` blast radius** — hiding rules changes every page hosting one, not just Journals | High | Surprise behaviour change | Increment 3 decides scope explicitly (§2.4) and the rung asserts the chosen scope on a non-journals page too |
| R5 | **Variant-precedence change (increment 4) has side effects** — priority 0 vs −1 is documented as load-bearing (`block_profile.yaml:118-124`) | Medium | Rules render wrong elsewhere; a `holon_sql` trigger could fall through to `query_result` and try to *execute* | Anti-overcorrection probe: after the change, assert a rule's trigger sibling still does **not** reach the display-query path (`has_query_source`, `yaml:34`) |
| R6 | **Two sources of truth for the journals seed** — `holon-frontend/src/lib.rs:101-140` and `assets/default/Journals.org` must stay byte-equivalent | Medium | Silent divergence; disk and boot disagree | Any change to the render spec edits **both**; `lib.rs:985-990` already pins one direction — extend it |
| R7 | **Scroll-window flapping** — a row oscillating across the boundary re-materialises repeatedly and burns the SLO | Medium | Jank | Hysteresis: expand margin > collapse margin. Rung drives a slow back-and-forth scroll and bounds the materialisation count |
| R8 | **Day pages are separate files** (`Journals/<date>.org`) — materialising many days may hit file I/O, not just SQL | Low–Medium | Latency worse than modelled | Increment 0 measures I/O alongside matview counts |
| R9 | **Windowing pressure to "just add LIMIT"** — the deferred plan text invites it | Medium | Broken snapshot/stream agreement, subtly wrong feed | §2.3(1); `util.rs:43-48` is the citation. Any future LIMIT proposal is a delivery-protocol change needing Martin's ruling |
| R10 | **Hidden rules become invisible-and-forgotten** — a user cannot find a rule to disable it | Medium | Usability regression, and worse than the current loud card | RULED (§8.3): hiding may land before the Rules page, but the disclosure toggle ships ATOMICALLY with it — no tree state has rules hidden with no reveal affordance. The collapsed marker must have real height (never `spacer(0)`). Increment 5 remains the full answer, still unruled |

---

## 6. RECOMMENDED ORDER

`0 (measure) → 1 (unblock navigation) → 2 (lazy scroll) → 3 (hide rules) → 4 (edit revealed rule)`;
**5 not built.**

3 and 4 are independent of 0–2 and can run in a parallel lane — they touch
`block_profile.yaml` and nothing 0–2 touches. 4 depends on 3.

---

## 7. STALENESS GUARD — re-run at the start of EVERY increment

Any changed answer invalidates the increment's premises; re-verify §1 before
proceeding.

```bash
# 0. Base sentinel (STOP with status stale-base on miss)
grep -q "stripe_of" crates/holon/src/api/operation_engine.rs

# 1. Journals seed still reads journal_feed with render list()
grep -n "FROM journal_feed" crates/holon-frontend/src/lib.rs           # expect :122
grep -n 'sortkey: "-content"' crates/holon-frontend/src/lib.rs          # expect :133
grep -n "FROM journal_feed" assets/default/Journals.org

# 2. LIMIT/ORDER-BY windowing contract still refuses SQL-side windows
grep -n "LIMIT\` / \`OFFSET\` are excluded" crates/holon-turso/src/util.rs   # expect :46
grep -n "fn strip_order_by" crates/holon-turso/src/util.rs                   # expect :36
grep -n "strip_order_by_also_strips_limit" crates/holon-turso/src/util.rs    # expect :420

# 3. expand_default still forces eager per-day materialisation
grep -n "expand_default" crates/holon-turso/sql/schema/journal_feed_matview.sql
grep -n "default_expanded: true" assets/default/types/block_profile.yaml

# 4. Rule-hiding hook: variant names + priorities unchanged
grep -n "name: rule_card\|name: source_editing\|name: embedded_page\|name: holon_source" \
  assets/default/types/block_profile.yaml
grep -n "is_rule_head:\|is_program:" assets/default/types/block_profile.yaml   # expect :45, :52

# 5. The navigation blocker is still open (if this is GONE, increment 1 may be done)
grep -n "journal feed render_source unreachable via focus" \
  crates/holon-integration-tests/src/pbt/frontend_slice/structural_pbt.rs

# 6. GPUI still virtualizes the collection and still filters rows
grep -n "list(self.list_state.clone()" frontends/gpui/src/views/reactive_shell.rs   # expect :924
grep -n "fn compute_visible_indices" frontends/gpui/src/views/reactive_shell.rs     # expect :667

# 7. Windowed scroll harness still available
grep -n "fn simulate_wheel_at" frontends/gpui/tests/support/mod.rs
```

---

## 8. RULINGS (Martin, 2026-08-11) — all four RULED, each per recommendation

These are decided. They are not open questions; the sections above are written
to match them.

1. **VIEW-LAYER WINDOWING — RULED.** `journal_feed` stays unbounded; the window
   is applied downstream of the CDC stream (§2.2). This **supersedes** the
   deferred-LIMIT note at `docs/Plans/JournalFeed-2026-07-18.md:80-81` and its
   "increment 2" line at `:95`, which must be amended in-repo as part of this
   work (increment 0 carries the doc edit — see §3 increment 0). Building a
   windowed delivery protocol (cursor in `QueryContext`, snapshot/stream
   reconciliation) is **out of scope** and would need its own ruling.
2. **HIDE ALL OF `is_program` — RULED.** Not `is_rule_head` alone. The rule head
   *and* its trigger sibling are hidden together (`block_profile.yaml:52`), so a
   rule is never half-hidden. This is a visible behaviour change on **every**
   page hosting a rule, not only Journals; increment 3's rung must assert the
   chosen scope on a non-journals page too (R4).
3. **HIDING MAY LAND BEFORE THE RULES PAGE — RULED.** Increment 3 is not
   blocked on increment 5. Binding condition: **the disclosure toggle ships
   atomically with the hiding**, in the same increment and the same commit —
   there is never a tree state where rules are hidden with no affordance to
   reveal them. This is the mitigation for R10 and an acceptance criterion of
   increment 3, not a follow-up.
4. **INCREMENT 1 IS IN-LANE, BUT TIMEBOXED — RULED.** Fix the navigation bug
   here. **STOP-AND-ESCALATE CRITERION:** if the repair cannot be contained to
   the journals/focus rendering path — i.e. if it requires changing the
   **delivery or navigation architecture** (the shape of focus→render
   delegation, the CDC delivery contract, `QueryContext`/`execute_query`
   signatures, or the region/tab navigation model) rather than a focused repair
   at the point of divergence — **STOP immediately, revert to a clean tree, and
   report for escalation to its own task.** Do not absorb an architecture change
   under this lane. Signals that the criterion is met: the fix touches
   `backend_engine.rs` query-delivery signatures; it changes how *any* surface
   other than the journals feed resolves its render source; or the smallest
   working repair requires more than a focused, single-seam change.
