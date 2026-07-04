# Entity-URI Links (F1) — design

Design pass for vault task `4ef71fd0` (ClaudeCode.org, "F1: resolve entity-URI
link targets in block_links + backlinks"). **No code in this pass.** Base rev
`d049523f`. Ratified upstream constraints (ClaudeCode.org `59e8ec0b`): link at
**agent grain**, session rollup derivable, and **do not hard-code the
orchestrator workflow** — semantics live in queries and profiles, the schema
stores structure only.

## Goals

- A block can link to a foreign integration entity — `cc-session:<id>`,
  `cc-agent:<id>`, and generically `<provider-entity>:<id>` — through the
  ordinary `[[…]]` surface.
- That link **round-trips through org text byte-stably**, appears in
  `block_links`, and shows up in the `backlinks` matview so the entity's page
  lists every block that mentions it.
- Clicking it navigates to a page for that entity.
- It is insertable from `[[` autocomplete.

## Non-goals (F1)

- No new provider. `cc_agent` is F2; F1 lands on `cc-session:` plus a synthetic
  test entity and is scheme-generic thereafter.
- No entity↔block identity adoption (no stub Page block standing in for a
  foreign entity). See OQ-6.
- No FTS / scoped search (F4), no context export, no distillation. F1 is the
  substrate those three sit on.
- No change to how foreign rows are fetched (FDW + `write_through` stays as
  ruled 2026-07-30).

## 0. Verified premises

Each of these was read at `d049523f`; the design leans on them.

1. **Link targets carry schemes; the bare-ID convention does not apply to
   them.** `ORG_SYNTAX.md`'s bare-ID rule governs `:ID:` drawers and
   `:REQUIRES:` values. Link targets are the exception and always were: the
   renderer writes `EntityRef::Internal { id } => id.as_str()`
   (`crates/holon-org-format/src/inline_marks.rs:410`) and the round-trip test
   at `inline_marks.rs:887` asserts `[[block:abc-123][see also]]` renders back
   verbatim.
2. **`EntityUri` already expresses foreign schemes.** It wraps an RFC 3986
   `Uri<String>`; `scheme()`/`id()` are generic
   (`crates/holon-api/src/entity_uri.rs:23,127,134`). `cc-session` is a valid
   scheme.
3. **Foreign cache rows already carry those exact URIs as their ids.**
   `id_scheme_for_entity(prefix, entity) = format!("{prefix}{entity}").replace('_',"-")`
   (`crates/holon-mcp-client/src/mcp_vtable.rs:284`) — entity `session` under
   `entity_prefix: "cc_"` yields scheme `cc-session`, matching
   `EntityName`'s `_`→`-` normalization (`crates/holon-api/src/types.rs:48`).
   The claude-history sidecar confirms it in SQL:
   `substr(cc_project.id, 12)` strips the literal `cc-project:` prefix
   (`docs/integrations/claude-history.yaml:108`).
4. **Profile resolution keys on the URI scheme.**
   `crates/holon-profiles/src/lib.rs:1030` — `EntityName::new(entity_uri.scheme())`.
   A `cc-session:` row already renders through the `session` profile the
   sidecar declares (`claude-history.yaml:41-50`).
5. **The `backlinks` matview never joins the target.**
   `backlinks_view_select()` (`crates/holon-turso/src/schema_modules.rs:918-929`)
   is `block_links bl JOIN block_raw b ON b.id = bl.source_block_id WHERE
   bl.resolved_id IS NOT NULL`, projecting `bl.resolved_id AS target_id`. The
   target appears only as a projected column and a NULL filter.
6. **The backlinks section already joins on a free-form id.** The seeded
   section SQL is `SELECT bl.* FROM backlinks bl JOIN focus_roots fr ON
   bl.target_id = fr.root_id` (`crates/holon-turso/tests/backlinks_section_matview.rs:24`),
   and `focus_roots` is a plain projection of `navigation_history` with no join
   to `block` (`crates/holon-turso/sql/schema/matview_focus_roots.sql`).
   `Schema.md`'s claim that `focus_roots` JOINs the `block` matview is stale —
   worth correcting when this lands.
7. **`block_links` rows are re-derived from marks on every block write.**
   `block_link_statements` issues `DELETE FROM block_links WHERE
   source_block_id = …` then re-INSERTs from `derive_block_links(&marks)`
   (`crates/holon/src/core/sql_operation_provider.rs:1019-1037`). The DDL has
   no CHECK on `kind` and no FK on `resolved_id`
   (`crates/holon-turso/sql/schema/block_links.sql`).
8. **The one gap is the classifier.** `classify_link` treats only a `block:`
   prefix as `Resolved`; every other non-URL target becomes a page creation
   intent (`crates/holon-api/src/link_parser.rs:227-280`). `[[cc-session:abc]]`
   today becomes a *page named `cc-session:abc`* hashed to a `block:` id.

The consequence of 5–8: the storage and backlink substrate is **already
entity-generic**. F1a is a parse-boundary change, not a schema change.

## 1. Org round-trip

### Syntax written to disk

```org
See [[cc-session:0f3a1c88-…][refactor the matview lease]] for the trail.
Bare form is legal too: [[cc-agent:7bd2…]]
```

Identical shape to the existing `[[block:<uuid>][label]]` form — same
delimiters, same `][` label split, same trimming rule. Nothing new is written
to disk that a human editing the file in Emacs would find surprising, and
nothing violates the bare-ID convention (premise 1).

### Parse boundary

`classify_link` gains one arm **before** the creation-intent fallback: if the
target's scheme parses and is a **registered entity scheme**, return
`LinkTarget::Resolved(EntityUri)`. Everything else is unchanged.

"Registered" must be a **closed set**, not a shape test. A shape test
(`^[a-z][a-z0-9+.-]*:`) would silently reclassify `[[Areas:Work]]` — a
perfectly legal page name — as an entity link. The closed set is the set of
`EntityName`s the schema/profile registry knows: built-ins (`block`, `tag`,
`person`) plus every entity declared by a YAML sidecar. That set is already
computed at startup; F1 projects it, it does not invent a second source of
truth. Hard-coding `cc-session`/`cc-agent` in `holon-api` is refused outright —
it would break the "no Rust code per integration" contract
(`docs/Architecture/Integrations.md:179-190`).
<!--
Not sure if this works. What happens if you add or worse remove an integration?
Then you would have links that suddenly are interpreted as page names.
What do you think about prohibiting `:` in page names?
It's probably problematic under Windows as well.
Your example `[[Areas:Work]]` I would rather write `[[Areas/Work]]`.
Otoh, one might have page titles like `Ketosis: How to loose weight`.
But I would assume we always have a space after the colon then and a capital letter for the word before the colon.
Alternatively we could prohibit links like `[[cc-session:acbde1234]` (which renders pretty awkwardly) completely and force the user to write
`[[cc-session:abcde1234][cc-session:abcde1234]]` if he really wants it.
Then the `[[...]]` would be reserved for pages.
But I think org mode allows `[[cc-session:abcde1234]]` so we have to deal with it anyway.
Let's do a top-down first principles session on this and talk about the external constraints
and what would even be possible within those constraints and what would happen if we break them.
-->

**RATIFIED (first-principles session, Martin 2026-07-31): three-state
classifier with a reserved scheme shape — replaces the closed-set-only design
above.**

External constraints that decide it:

- Org-mode itself has typed links: `scheme:path` against a registry
  (`org-link-parameters`); Emacs already treats `[[cc-session:abc]]` as
  link-typed, never as a heading name. A scheme registry mirrors org, it does
  not invent.
- RFC 3986 scheme shape is `letter (letter|digit|+|-|.)* :` with **no
  spaces** — so `Ketosis: How to lose weight` is structurally not a scheme
  (colon-space), no capitalization heuristics needed.
- Windows forbids `:` in filenames, so a scheme-shaped page *name* could never
  be a portable page file anyway.
- Vault measured 2026-07-31: zero scheme-shaped page names (all 7 colon names
  are colon-space "Phase N:" titles), zero scheme-shaped link targets besides
  `block:` and org-native `id:`. The reservation is empirically free.

The classifier:

1. Scheme-shaped target + **registered** scheme → `Resolved` entity link.
2. Scheme-shaped target + **unregistered** scheme → **UnknownScheme**:
   rendered muted/disclosed, never a page-creation intent. The scheme shape is
   reserved.
3. Not scheme-shaped (no colon, or colon followed by space) → page semantics,
   unchanged.

Two fail-loud guards: page creation rejects scheme-shaped names ("use `/` for
hierarchy"); integration registration scans for pre-existing colliding page
names and refuses loudly.

Why this kills the config-fragility: removing an integration degrades its
links Resolved → UnknownScheme (disclosed, bytes untouched, restored on
re-registration) — never reinterpreted as page names. Adding one flips
UnknownScheme → Resolved — safe because the reservation guaranteed no page was
silently created under that shape meanwhile. Classification can only ever move
*within* the entity-link interpretation, never across the page/entity
boundary. Bare `[[cc-session:abc]]` stays legal; display handles the
awkwardness (profile title when cached, muted URI when not).

Plumbing: `classify_link` becomes a method on a `LinkTargetClassifier` value
carrying the scheme set, constructed once at each parse boundary (org parser,
`inline_marks::strip_link`, `link_provider`). `Default` carries the built-ins
so the pure unit tests and the reference model stay IO-free. See **OQ-1** for
the alternative.

### Collision rules

| Target | Classified as | Why |
|---|---|---|
| `cc-session:abc` (scheme registered) | `Resolved(cc-session:abc)` | closed-set hit |
| `Areas:Work` (scheme not registered) | `CreationIntent` → page | unchanged behavior; the closed set is what makes this safe |
| `cc-sesion:abc` (typo) | `CreationIntent` → page | degrades to today's dangling-page UX, visibly distinct from a resolved link |
| `block:abc` | `Resolved` | unchanged |
| `https://…`, `mailto:` | `External` | unchanged, checked first |

The typo case is deliberately **not** an `Err`. Org files are hand-editable;
failing the parse would make one typo unrenderable. The disclosure is visual:
it renders as a dangling page link, which already looks different from a
resolved one. This is the single carve-out from fail-loud in this design, and
it is a carve-out toward *existing* behavior, never toward fabricated data.

### Render boundary

No change. `EntityRef::Internal { id } => id.as_str()` already emits the full
schemed URI. Round-trip is byte-stable by construction.

## 2. Storage

### Chosen: `resolved_id` carries the full entity URI; no second matview, no
### second table

The option the brief called (i), minus its second matview — which premise 5
shows is unnecessary.

`block_links` DDL is unchanged. `derive_block_links` changes one line:
`EntityRef::Internal { id }` currently maps every non-`tag` scheme to
`LinkKind::Block` (`crates/holon-api/src/inline_mark.rs:309-317`); it gains a
third case so a scheme outside `{block, tag}` becomes a new
**`LinkKind::Entity`** (`"entity"`). `resolved` stays `Some(id)` — an entity
link, like a `block:` link, resolves trivially at parse time.

### Why a new `kind` rather than reusing `'block'`

Functionally, `'block'` would work today: the two SQL sites that discriminate on
kind are the page re-resolution sweep (`kind = 'page'`,
`sql_operation_provider.rs:1260`) and the delete-time dangling sweep (matches on
`resolved_id` value, `:973`), and neither can collide with a `cc-` URI. But the
column's documented domain says `'block'` means "the target is a block id"
(`block_links.sql:11-14`), and every future consumer — scoped search, export —
will want to select entity links without string-matching schemes. Storing a lie
now to save one enum variant is the kind of thing "parse, don't validate" exists
to prevent.

### Why not a parallel `entity_links` table (option ii)

`block_links` is touched by at least eight write paths that would each need a
sibling: derive+insert, delete-time dangling clear, page re-resolution, undo
capture/restore of prior `resolved_id`, `merge_blocks` inbound rewrite, the
`rewrite_link_resolution` op, and the sharing alias ledger
(`crates/holon-sharing/src/alias_ledger.rs:254`). That is a fork of the entire
link lifecycle — including undo correctness — bought for zero query benefit,
since the junction is already polymorphic in practice (premise 5).
<!--
I just remember that we already have the capability to wrap non-block in a block.
Did you see that?
Might be suitable here as well.
-->

**Answer (2026-07-31):** three distinct wrap mechanisms exist, and they split:

- *Schema permissiveness* — `block_raw.id` accepts any URI, so a persistent
  wrapper block per entity is representable. We used exactly this for
  Directory entities and purged it (it broke GPUI boot): a block row that is
  not a real block violates the doc-root / org-write-back / Loro-presence /
  ingest assumptions, which is what the foreign-inline quarantine machinery
  guards against. A wrapper would also need a minting/deletion lifecycle
  driven by link derivation — the same lifecycle-fork argument as
  `entity_links`, pointed at the core table.
- *Query/live blocks* — wrap foreign data at render time. This IS used here:
  the §5 entity page is one cache row + its profile + backlinks, through the
  existing profile render path. No persistent wrapper needed.
- *Transclusion* — content-level, mints no identity; orthogonal.

The two things a persistent wrapper would buy (backlinks, click-through) are
already free/cheap in this design; its one genuine payoff — entities as
first-class *organizable* outline citizens (draggable, taggable, annotated) —
is a deliberate explicit-user-action feature (a bookmark block whose content
links the entity), composable later, not link plumbing. **Ruled: no wrapper
blocks in F1.**

### Why not polymorphic-with-kind-driven-joins (option iii)

That option only pays for itself if some matview must JOIN the target to
hydrate it. None does. `backlinks` hydrates the **source** block; the target is
a bare id column. Adding kind-driven join branches would introduce exactly the
matview-on-matview and per-kind-view proliferation the schema deliberately
avoids.

### IVM / CDC analysis

- `backlinks` reads `block_links` and `block_raw`, both **base tables** — no
  matview-on-matview, so the chained-matview hazard is untouched.
- The change adds no new view, no new column, and no new dependency edge, so
  the DBSP graph shape is identical. Maintenance stays O(delta) over the same
  inputs.
- The one behavioral delta inside IVM: rows whose `resolved_id` is an entity
  URI now pass the `IS NOT NULL` filter where before they were NULL (dangling).
  That is an ordinary insert-delta on an existing operator, not a new operator.
- CDC continues to flow only through matviews. Entity backlinks reach the UI on
  the same `backlinks` stream as block backlinks — no second stream, no second
  subscription, no new lease.

### Migration

**None.** No DDL change; the `kind` domain widens but the column has no CHECK,
and every row is re-derived from marks on the next write of its source block
(premise 7). Pre-F1 databases cannot contain an entity-URI link at all, because
the classifier could not mint one — so there is nothing to backfill and nothing
to rewrite.

## 3. Resolution semantics

Resolution and presence are **two different questions**, and conflating them is
where a design like this usually goes wrong.

**Resolution is parse-time and total.** An entity-URI target is an id-form
target. `resolved = Some(uri)` with no lookup, exactly as `block:` does today.
An entity link is therefore *never dangling* and never depends on whether the
FDW has been primed. `block_links.resolved_id` is populated the moment the
block is written.

**Presence is a display concern with three disclosed states:**

| State | Condition | Rendering |
|---|---|---|
| Present | row exists in the cache table for that scheme | label = the explicit `[[…][label]]` text, else the profile's title column; normal link styling |
| Pending | scheme registered, row not in cache (FDW not primed for it yet) | the URI itself, muted, with a "not yet fetched" affordance |
| Unregistered scheme | not reachable — the classifier never mints a Resolved target for one | (renders as a dangling page link, §1) |

Pending is a **disclosed fallback** (error-philosophy priority 2), not an
error: the FDW is on-demand by design and a partial cache is the normal steady
state, not a degradation. What is forbidden is priority 4 — showing a plausible
but fabricated title, or silently showing the raw URI as if it were the title.
The visual distinction is what makes it honest.

Deliberately **not** done: eager priming on render. A link in a scrolled-past
block must not trigger an MCP fan-out. Priming stays where it is — the entity
page (F1b) issues the read.

## 4. Backlinks for entities

Zero changes. The `backlinks` matview projects `resolved_id AS target_id`
(premise 5) and the section query joins `bl.target_id = fr.root_id` (premise
6). Focusing `cc-session:abc` therefore lists every block linking to it, on the
existing stream, through the existing seeded query.

This is the single strongest argument for the chosen storage option: the
feature's headline capability — "high-level tasks carry attribution to the
sessions that worked on them, visible from the session" — falls out of a
one-arm classifier change plus one enum variant.

## 5. Click-through and the entity page

**Navigation carries an untyped id today, and that is enough.**
`navigation.focus { region, block_id }` passes a `Value::String`
(`crates/holon-frontend/src/reactive.rs:3958-3969`), `navigation_history.block_id`
is TEXT, and `focus_roots` does not join `block`
(`matview_focus_roots.sql`). Focusing `cc-session:abc` is representable
end-to-end with no schema or op change.

**What breaks is the main panel.** It expands `CHILD_OF*0..N` from `root_id`
over blocks; an entity root has no block row, so the panel renders empty.

**The minimal extension: branch the main panel on the focus root's scheme.**

- `scheme == "block"` → today's block-tree page, unchanged.
- otherwise → an **entity page**: `render_entity()` over the single row
  `SELECT * FROM <scheme with '-'→'_'> WHERE id = '<uri>'`, followed by the
  existing backlinks section.

No new page-template machinery is needed, and that is the point. Profile
resolution already keys on the scheme (premise 4); the claude-history sidecar
already ships a `session` profile whose render is an `expand_toggle` over
`cc_message_fdw` (`claude-history.yaml:43-50`). An entity page is therefore
"one row + its profile + backlinks" — a parameterized *query*, not a
parameterized *template*. This also honors the ratification: the semantics live
in the profile and the query, and the schema stores structure only.

The `block_id` param name becomes a misnomer. See **OQ-4**.

## 6. Autocomplete (F1c)

`search_link_candidates` (`crates/holon/src/api/query_engine.rs:76-94`) is a
UNION ALL of two block branches returning `LinkCandidate { id: EntityUri, label
}`. F1c adds one branch per entity that declares a `link_candidates` block in
its YAML sidecar (a search column and a label column), e.g.
`SELECT id, first_prompt FROM cc_session WHERE first_prompt LIKE ?`.

The insertion side needs nothing: `on_select` already emits
`format!("[[{}][{}]]", item.id, item.label)`
(`crates/holon-frontend/src/link_provider.rs:116-126`), which for a candidate
whose id is `cc-session:abc` produces exactly the syntax §1 specifies.

**Decisive constraint: search the cache table, never the `_fdw` table.** A
keystroke must not fan out MCP reads. The consequence — autocomplete offers only
already-cached entities — is disclosed with a group header, not hidden.

## 7. Red-first test plan

Per the `holon-feature` contract: model first, red for the right reason, then
green. New transitions land in
`crates/holon-integration-tests/src/pbt/transitions/` and register in
`transitions/mod.rs` (`declare_e2e_transitions!`); new invariants land in
`src/pbt/composed/invariants/` with the positive/negative-containment/catch
triad and one line in `composed/catalog.rs`.

### Seeding — the one real test-design decision

The keystone must not depend on the claude-history MCP server: it is not in
`wiring_axes()`, it needs `~/.claude`, and it would make a hermetic test
machine-dependent. Instead seed a **synthetic foreign entity** (`t-widget`) —
its own tiny cache table and profile, declared the same YAML way a real
integration is. This tests the *generic* mechanism, which is exactly what the
ratification asks for. See **OQ-3**.

### F1a — headless keystone

New transition `InsertEntityLink`: type `[[t-widget:<seeded-id>][label]]` into
a drawn block through the existing text-edit rung; the reference model records
the link.

| Invariant | Asserts | Expected red today |
|---|---|---|
| `inv-entity-link-round-trips-org` | content → org render → re-parse yields the same mark set with `EntityRef::Internal` | re-parse yields `EntityRef::Name{"t-widget:…"}` — the classifier gap |
| `inv-entity-link-backlink-visible` | `backlinks WHERE target_id = '<uri>'` contains the source block | `resolved_id` is a hashed `block:` id, so `target_id` never equals the URI |
| `inv-link-kind-matches-target-scheme` | `block_links.kind = 'entity'` iff the target scheme is a registered non-block/tag entity | row is `kind='page'`, `resolved_id` NULL |

All three fail on an assertion, not a compile error — the transition compiles
and runs against today's code because it only types text.

### F1b — GPUI windowed

`inv-entity-focus-renders-profile`: after a windowed click on an entity link,
the main panel shows the entity profile's render output and **not** the
empty-page fallback; the backlinks section lists the source block. Red today:
empty panel.

The click path itself (`rendered_text.rs:273-286` → `nav_focus`) needs the
windowed tier because the ratified rule is that anything the user sees or
touches gets the windowed check, and "clicking a link lands on a non-empty
page" is precisely that.

### F1c

Headless `inv-entity-link-candidate-offered` over `search_link_candidates`,
plus a windowed popup check that selecting the candidate inserts the schemed
form.

### Gate

Each increment: `just pbt general` (teed), the new invariants green, then a
`dogfood-explorer` pass before it is called done.

## 8. Increment cut

**F1a — linkable + backlinked (the substrate).** Classifier scheme set +
`LinkTargetClassifier` plumbing; `LinkKind::Entity`; `derive_block_links` arm;
synthetic test entity seed. No storage change. Done when the three headless
invariants are green and a hand-authored regression shows an entity link's
source block appearing in `backlinks` for the entity URI.

**F1b — entity page + click-through.** Main-panel scheme branch; entity-page
query; pending-state rendering. Done when the windowed invariant is green and a
dogfood click on a `cc-session:` link lands on a page showing the session and
its backlinks.

**F1c — autocomplete.** `link_candidates` sidecar field; the extra UNION
branches; the "cached only" disclosure. Done when the headless + windowed
candidate checks are green.

F1a is independently useful: it makes attribution *queryable* (the input scoped
search and context export actually need) even before it is clickable.

## 9. Open questions — MARTIN

**OQ-1 — how the registered-scheme set reaches the classifier.**
(a) Explicit `LinkTargetClassifier` value threaded through each parse boundary:
honest, testable, but touches every `extract_inline_marks` caller.
(b) A process-global `OnceLock<BTreeSet<EntityName>>` seeded at startup: a
two-line change, but a hidden global that the PBT and every unit test must
remember to seed — and a forgotten seed degrades silently to "everything is a
page", the exact failure mode this design is trying to eliminate.
*Recommendation: (a).* The plumbing is mechanical and one-time; the silent
degradation in (b) is unbounded.
<!-- (a) -->

**OQ-2 — `LinkKind::Entity` as a fourth kind, or reuse `Block`.**
Reuse costs nothing today and saves a variant; a distinct kind keeps the
column's domain honest and gives downstream consumers a clean predicate.
*Recommendation: distinct kind.*

**OQ-3 — keystone seeds a synthetic entity, or wires the real claude-history
integration.** Synthetic keeps the keystone hermetic and tests the generic
mechanism; real gives end-to-end confidence but makes the keystone depend on a
local MCP binary and `~/.claude`.
*Recommendation: synthetic in the keystone; a separate non-keystone
integration test may exercise the real sidecar.*

**OQ-4 — rename `navigation.focus`'s `block_id` param (and the
`navigation_history.block_id` column) to `target_id`.** It is now a misnomer.
Renaming touches a persisted column, an op signature, and every frontend.
*Recommendation: keep the wire name for F1, document the widened meaning,
revisit if a second non-block focus target appears.*

**OQ-5 — F1 does not block on F2.** The ratification is "link at agent grain",
but `cc_agent` does not exist yet. F1 lands scheme-generic against
`cc-session:` and the synthetic entity; `cc-agent:` works the day F2 declares
the entity, with no F1 change. *Confirm this sequencing.*

**OQ-6 — should an entity page be addressable as a block?** i.e. a stub
`Page`-tagged block adopting the foreign entity's identity, so a user can write
notes "on" a session. The `canonical_entity`/`entity_alias` tables exist as
exactly this seam, and the sharing mount is a precedent for identity adoption.
But it is a large design (identity minting, org projection, consolidator
implications) and F1 does not need it — notes attach to a session today by
linking to it, and the backlinks section shows them.
*Recommendation: NO for F1; log as a separate topic in the vault.*

## 9b. RATIFIED rulings (Martin, 2026-07-31)

- **OQ-1: (a)** — explicit `LinkTargetClassifier` value, no hidden global.
- **OQ-2/3/4/5**: as recommended (distinct `LinkKind::Entity`; synthetic
  entity in the keystone; keep the `block_id` wire name, document the widened
  meaning; F1 does not block on F2).
- **OQ-6: NO** — and it now derives from the ratified ownership principle
  below rather than standing alone.

### The embed-vs-wrap hyper-cube (first-principles session, ratified)

Six seeming axes (identity, tree membership, annotation surface, data
authority, persistence class, lifecycle coupling) collapse under one
principle: **every piece of state lives in the store whose owner authors
it.** User-authored state about an entity (position, tags, notes, extra
properties) must live where user state lives — org+Loro — or it silently
becomes second-class (no sync, no re-ingest survival, invisible to sharing).
Provider-authored state stays in the cache or you get two-copies-who-wins.
Three points survive:

- **P1 — link-only (THIS design).** The entity URI is a join key: link
  target, backlink target, query key. Annotations are blocks that link to the
  entity; the backlinks section is the aggregator. Landed by F1.
- **P2 — wrapper block (sanctioned, deferred past F1).** A normal block
  referencing the entity — marked by an `entity:: <uri>` property (queryable,
  zero schema change), optionally rendering an embedded view. All block ops
  apply trivially because it IS a block — and they act on *your reference to*
  the entity, never the entity, which is the only coherent semantics and
  makes the ownership boundary visible. Nearly free once F1 lands
  (block + link + existing embed/live_block render). An explicit user action
  ("bookmark this session"), never minted implicitly by link derivation.
- **P3 — identity-aliased shell (RULED OUT).** A `block_raw` row whose id is
  the entity URI, structural fields only, data joined from cache. Its
  user-authored structural state is org/Loro-homeless (the Directory-entity
  purge is the scar), delete/undo semantics are unresolvable against the
  provider lifecycle, and direct annotation dies on refetch — so it needs a
  side table that re-invents P2 in a worse store. Dominated on every axis it
  claims to win. Entities appearing *inside* outline views is already covered
  virtually by query blocks (render-time membership, no storage identity).

## 10. Doc updates this lands with

- `docs/Architecture/Schema.md` — `block_links` row (kind domain gains
  `entity`; `resolved_id` explicitly cross-scheme), and the stale claim that
  `focus_roots` JOINs the `block` matview.
- `docs/Reference/ORG_SYNTAX.md` — a short section stating that link *targets*
  carry schemes (the documented exception to the bare-ID rule) and listing the
  entity-URI form.
- `docs/Explanation/DESIGN_LINKS.md` — it still describes the retired
  `block_link` table and "no backlinks matview"; both are false at `d049523f`.
  Out of F1's scope to rewrite, but the staleness should be flagged.
