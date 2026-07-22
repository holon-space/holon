# Tabs (PR #62 + #65) ↔ landed-main seed reconciliation

Base of the #62+#65 stack = OLD main `af0cdb4f`. Weave target = current main
`38d581090ac5` (bookmark head `850a7cd9`). This file gives the orchestrator the
EXACT final content of every conflicted `assets/default/index.org` block, plus
the test fixups applied in the workspace.

Chosen main-panel query shape: **hand-written `holon_sql` guarded recursive CTE**
(candidate 2), NOT `holon_gql` (candidate 1). Rationale below.

---

## Why holon_sql, not holon_gql (candidate decision + graph-schema finding)

The main panel must render ONLY the ACTIVE tab. With tabs, `focus_roots` holds
every open tab for the region; the active one is `navigation_cursor(region,
history_id)`. So the query needs a `focus_roots ⋈ navigation_cursor` equi-join
on `(region, history_id)`.

- **`navigation_cursor` is NOT a registered graph node.** Only `current_focus`
  and `focus_root` are registered (`crates/holon-turso/src/schema_modules.rs`
  `NavigationSchemaModule::graph_contributions`; `focus_root` at line ~596).
  I proved empirically (compiling the candidate GQL through the real engine's
  `compile_to_sql`) that a GQL `MATCH (nc:navigation_cursor) …` silently falls
  back to the generic EAV resolver — it compiles to a monster join over
  `nodes / node_labels / node_props_text|int|real|bool|json` and reads NONE of
  the real `navigation_cursor` table. This is exactly the "unregistered NODE
  labels fall back silently" hazard. Making GQL work would require registering
  `navigation_cursor` as a `MappedNodeResolver` node (a schema change with the
  di::registration smoke-test blast radius).
- A hand-written `holon_sql` CTE expresses the cursor equi-join directly, needs
  no schema change, and is precedented (right-sidebar/main-panel were both
  hand-written recursive CTEs before main's GQL swap).

### The delivery landmine and its real cause (proven, not guessed)

The pre-#62 (and #62's) main-panel CTE carried `b.*` (full block rows) THROUGH
the recursive `UNION ALL`, plus a `LEFT JOIN block_tags` page-stop, with NO
depth cap and NO visited-path guard. At ~70-file scale the Turso IVM matview
never delivered (60-90s hang / watch expiry). I reproduced the hang in a
synthetic ~70-page / ~1080-block vault: it TIMED OUT (>120s).

I compiled main's working GQL (`compile_to_sql`) to see WHY GQL delivers and
found the load-bearing shape: **the recursive CTE carries only IDs**
(`node_id, source_id, depth, visited`) and the OUTER query hydrates block
columns via `JOIN block d ON d.id = <cte>.node_id`. Carrying only 4 scalars
through the DBSP recursion is what makes IVM maintenance cheap. Mirroring that
structure, the same synthetic vault delivers in **~0.55s** (see delivery test).

Two additional Turso-IVM constraints I hit and encoded:
1. **Never ALIAS the recursive CTE in the outer/recursive references.** `JOIN
   focus_descendants fd ON fd.source_id = …` fails DDL with
   `Join condition column 'source_id' not found in either input`; referencing
   the CTE by its bare name (`JOIN focus_descendants ON
   focus_descendants.source_id = …`) succeeds. Main's compiled GQL never
   aliases the CTE — this is a hard IVM requirement, not a style choice.
2. Equi-join ON shapes only: join keys in `ON`, the `region = 'main'` constant
   in `WHERE` (the equi-join-storm lesson). The `navigation_cursor` join is
   `ON nc.region = fr.region AND nc.history_id = fr.history_id`.

Delivery numbers (synthetic 70 pages, ~1080 blocks, 5 open tabs, active=doc_0):
- unguarded `b.*` form: **TIMEOUT >120s**
- final ID-only guarded form: **create+first-read ≈ 0.55s**, 35 rows = exactly
  doc_0's depth-capped subtree (deep chain of 30 truncated at depth 20; other 4
  open tabs excluded by the cursor filter).

---

## Final content of every conflicted index.org block

### Block 1 — `left_sidebar::render::0`  (language: `render`)  — MERGE
Main changed `sortkey → col("content")` and added the `#{empty: "(untitled)"}`
placeholder; #62 added `cmd_action`/`ctrl_action` (modifier-click → open tab).
Take BOTH:

```
column(tree(#{parent_id: col("parent_id"), sortkey: col("content"), item_template: selectable(row(icon("notebook"), spacer(6), text(col("content"), #{empty: "(untitled)"})), #{action: navigation_focus(#{region: "main", block_id: col("id")}), cmd_action: navigation_open_tab(#{region: "main", block_id: col("id")}), ctrl_action: navigation_open_tab(#{region: "main", block_id: col("id")})})}), divider(), row(icon("sync"), spacer(6), text("Integrations", #{bold: true})), live_query(#{sql: "SELECT provider_name, updated_at FROM sync_states ORDER BY provider_name ASC", item_template: row(spacer(6), icon("link"), spacer(6), text(col("provider_name")), spacer(8), text(col("updated_at")))}))
```

NOTE: the `#{empty: "(untitled)"}` render-DSL option and `sortkey col("content")`
are MAIN-side changes and are NOT applied in this workspace (its base `af0cdb4f`
lacks the `empty:` render support). In the workspace this block stays at #62's
form (`sortkey: col("sort_key")`, `text(col("content"))`, + the two modifier
actions). The block above is the WEAVE-TIME target against real main.

### Block 2 — `default-main-panel::render::0`  (language: `render`)  — #62 superset
Main's backlinks base is unchanged from `af0cdb4f`; #62 adds the cursor
equi-join so the Linked-references section tracks the ACTIVE tab, not a union
over all open tabs. This is exactly #62's workspace line (already applied):

```
column(collection_view(), divider(), row(icon("link"), spacer(6), text("Linked references", #{bold: true})), live_query(#{sql: "SELECT bl.id AS id, bl.content AS content, bl.parent_id AS parent_id FROM backlinks bl JOIN focus_roots fr ON bl.target_id = fr.root_id JOIN navigation_cursor nc ON nc.region = fr.region AND nc.history_id = fr.history_id WHERE fr.region = 'main' ORDER BY bl.content ASC", item_template: selectable(row(icon("orgmode"), spacer(6), text(col("content"))), #{action: navigation_focus(#{region: "main", block_id: col("id")})})}))
```

### Block 3 — `default-main-panel::src::0`  (language: `holon_sql`)  — RECONCILED (this task)
The guarded ID-only recursive CTE + cursor equi-join. THIS is the landmine fix
and is applied in the workspace. Proven to deliver (~0.55s) and IVM-maintainable:

```
#+BEGIN_SRC holon_sql :id default-main-panel::src::0
WITH RECURSIVE focus_descendants AS (
  SELECT b.id AS node_id, b.id AS source_id, 0 AS depth, CAST(b.id AS TEXT) AS visited
  FROM block b
  JOIN focus_roots fr ON b.id = fr.root_id
  UNION ALL
  SELECT child.id, focus_descendants.source_id, focus_descendants.depth + 1, focus_descendants.visited || ',' || CAST(child.id AS TEXT)
  FROM focus_descendants
  JOIN block child ON child.parent_id = focus_descendants.node_id
  WHERE focus_descendants.depth < 20
    AND ',' || focus_descendants.visited || ',' NOT LIKE '%,' || CAST(child.id AS TEXT) || ',%'
)
SELECT d.*
FROM focus_roots fr
JOIN block root ON root.id = fr.root_id
JOIN focus_descendants ON focus_descendants.source_id = root.id
JOIN block d ON d.id = focus_descendants.node_id
JOIN navigation_cursor nc ON nc.region = fr.region AND nc.history_id = fr.history_id
WHERE fr.region = 'main'
#+END_SRC
```

Notes for the weave:
- Language is `holon_sql` (NOT `holon_gql`). Main-panel diverges from the
  right-sidebar's `holon_gql` deliberately — only main-panel needs the
  cursor filter, which GQL cannot express without registering a new node.
- No `ORDER BY` (matview strips it anyway; `collection_view()`/tree sorts by
  `sort_key`). Matches main's main-panel GQL which also `RETURN d` unordered.
- Drops the old `LEFT JOIN block_tags` page-stop — matches main's GQL, which
  also dropped it in favour of the depth cap.

### Block 4 — `default-right-sidebar::src::0`  (language: `holon_gql`)  — TAKE MAIN
#62 did NOT touch the right sidebar (the diff shows only property-line
re-chunking noise). Main rewrote it to the anchored-varlen GQL. Take main's
verbatim (right sidebar shows ALL pinned pages — no cursor filter, correct):

```
#+BEGIN_SRC holon_gql :id default-right-sidebar::src::0
MATCH (fr:focus_root), (root:block)<-[:CHILD_OF*0..20]-(d:block) WHERE fr.region = 'right' AND root.id = fr.root_id RETURN d ORDER BY fr.added_ts DESC, d.sort_key
#+END_SRC
```

In the workspace this block is still the OLD `holon_sql` recursive form (from
`af0cdb4f`); leave it — the weave replaces it with main's GQL above. It is NOT
part of #62's or this reconcile's scope.

Unchanged / no-conflict blocks (identical both sides): `left_sidebar::src::0`,
`default-right-sidebar::render::0`, the advice-rule block.

---

## Fixup diff summary (what changed in the workspace)

1. `assets/default/index.org` — `default-main-panel::src::0` replaced with the
   Block-3 guarded ID-only CTE above (the landmine fix). Blocks 1/2/4 left at
   their workspace form per the notes above (main-side improvements deferred to
   the weave).
2. `crates/holon-app/tests/backlinks_section_seed.rs`
   - `SECTION_SQL` const rebound to the shipped cursor-joined backlinks query
     (adds `JOIN navigation_cursor nc ON nc.region = fr.region AND
     nc.history_id = fr.history_id`).
   - `focus_main()` helper now also sets `navigation_cursor` to the just-opened
     row (mirrors focus_replace's cursor move) so the cursor-joined section
     query resolves the active tab. Without this the cursor join returns empty.
   All 3 guards pass.
3. `crates/holon/tests/turso_storage_repros/tabs_main_panel_delivery.rs` (NEW)
   + registered in `.../turso_storage_repros/main.rs`. The vault-scale delivery
   guard (~70 pages, 5 open tabs): asserts create+first-read < 5s (measured
   ~0.55s) and that the panel contains EXACTLY the active tab's depth-capped
   subtree.

## Untouched (verified independent)
- #65 cursor-follow close logic (`NavigationProvider`, `left/right_neighbor_open_tab.sql`,
  `get_row_region_and_cursor.sql`): depends only on `navigation_history` +
  `navigation_cursor`, NOT on the main-panel src shape. Both `tabs_close_cursor_follow`
  and `tabs_cursor_filtered_panel` storage repros use their OWN inline
  `main_panel` matview proxy (cursor equi-join, non-recursive) and are unaffected
  by the src::0 text.
- `crates/holon-turso/tests/recursive_cte_page_boundary.rs`: inlines the OLD
  page-stop CTE and claims index.org fidelity. It is IDENTICAL on main and in
  the workspace (main did NOT update it when it swapped index.org to GQL), so it
  is a pre-existing fidelity/coverage gap, NOT a #62/#65 conflict — out of scope.
  Flag for a follow-up: rebind or delete it (its "mirrors index.org" claim is
  stale on main already).
