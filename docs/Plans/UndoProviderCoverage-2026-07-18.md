# Undo / Inverse-Provider Coverage — 2026-07-18

*Inventory doc. Gates the block→page transform (Option-B/C undo-atomicity open
question, `BlockToPageTransform-Options-2026-07-17.md` Q5). Every load-bearing
claim is cited `file:line`. Architecture settled by the undo ruling
(2026-07-10, "A shaped for C"); U1 landed. The remaining gap is **provider
coverage** — which operations can describe their inverse.*

## How undo works here (the provider contract)

- An `UndoEntry` stores `ops` (forward) and `inverse_ops` (inverse), **frozen at
  execution, never recomputed** — `crates/holon-core/src/undo.rs:131,134`. The
  `Vec` shape is deliberate "so a future compound split/join is one entry"
  (`undo.rs:131`). **A composite transform can therefore be one undo entry —
  but only if every constituent op yields an inverse.**
- Each operation returns an `OperationResult` whose `undo: UndoAction` field is
  the *provider* — `crates/holon-core/src/traits.rs:351`. `UndoAction` has three
  variants (`traits.rs:218-227`):
  - `Undo(Operation)` — a concrete inverse op (the real provider).
  - `DeclaredIrreversible(&'static str)` — a *named* refusal (fail-loud: no undo
    entry is pushed, and it says why).
  - `Undeclared` — the loud-failure default; reaching the engine still
    `Undeclared` is a programming error.
- Replay is precondition-fingerprinted on projected `(entity, field)`
  `FieldDelta`s (`undo.rs:83-118`): the stale-guard re-verifies live state
  before replaying an inverse. An op that returns **empty `changes`** disables
  the stale-guard for that step (single-writer-safe only) — this bit us as a P1
  data-loss (`loro_block_operations.rs:386-389`).
- Structural ops close the coalescing group and stand alone (`undo.rs` push
  path); only single-char text edits on one `(entity, field)` coalesce.

**Exactness vocabulary used below**
- **exact** — inverse restores the pre-op state faithfully (byte/id/set-level).
- **conditional** — exact in the common (leaf / genuine-insert) case; a
  structural edge case is `DeclaredIrreversible` (fail-loud, never lossy).
- **absent** — always `DeclaredIrreversible`/`irreversible`: no inverse at all.

## Two write authorities — delete diverges by mode

CRUD is served by **different providers depending on mode**, and their undo
behaviour is **not** the same:

- **Full / Loro mode (default prod):** `LoroBlockOperations` is the CRUD
  authority (`sql_block_operations.rs:960-963` routes user CRUD to it).
- **SqlOnly mode:** the generic `SqlOperationProvider` serves CRUD.

The structural chord ops (`move_block`, `split_block`, `join_block`, `indent`,
`outdent`, `move_up`, `move_down`, `embed_entity`) are the **shared default trait
impls** in `traits.rs`, so both providers behave identically there. **`delete`
is where they split** (see table).

## Coverage table

| Op (dispatched name) | Provider (inverse) | Exactness | Notes / file:line |
|---|---|---|---|
| `set_field(content=String)` | `set_field` restoring prior text | **exact** | `loro_block_operations.rs:397-417` |
| `set_field(content=Object{marks})` | `set_field(content, Object{prior text+marks})` | **exact** *(NEW)* | whole-set restore via `update_block_marked`; text delta arms the guard only when text changed (else empty, single-writer) — `loro_block_operations.rs` (this change) |
| `set_field(marks)` | `set_field(content, Object{prior text+marks})` | **exact** *(NEW)* | atomic text+marks restore; empty precond (text unchanged) — `loro_block_operations.rs` (this change) |
| `set_field(task_state)` | `set_field(task_state, old)` | **exact** | empty precond (property blob, single-writer) — `:418-431` |
| `set_field(edge: tags/requires/…)` | `set_field(field, prior_set)` | **exact** | whole-set restore — `:433-447` |
| `set_field(other property: DEADLINE/PRIORITY/generic)` | — | **absent** | `_ => (None, empty)` — `loro_block_operations.rs:449` |
| `create` | `delete(id)` | **conditional** | exact when inserted; insert-ignored irreversible — Loro `:742-766`, Sql `:1809-1832` |
| `delete` — **Loro authority (default)** | — | **absent** | unconditionally irreversible — `loro_block_operations.rs:771-782` |
| `delete` — SqlOnly authority | `create(full row+edges)` | **conditional** | leaf exact (identity-preserving); cascade/absent irreversible — `sql_operation_provider.rs:1885-1940` |
| `update` | inherits `create` | **conditional** | Loro upsert→create result (`:790-793`); Sql `update` arm always irreversible (`:1883`) |
| `split_block` | `restore_join` | **exact** | recreates merged-away block, resets sibling content — `traits.rs:1294-1306` |
| `join_block` | `restore_split` | **conditional** | LEAF exact; merged-block-with-children / pos≠0 / case-B irreversible — `traits.rs:1482-1512` |
| `move_block` | `move_block(old_parent, old_pred)` | **exact** | restores parent + predecessor — `traits.rs:1012-1024` |
| `indent` | `move_block(...)` | **exact** | delegates to move_block — `traits.rs:902-904,1060-1071` |
| `outdent` | `move_block(...)` | **exact** | `traits.rs:1060-1071` |
| `move_up` | `move_block(...)` | **exact** | absolute restore — `traits.rs:1762-1773` |
| `move_down` | `move_block(...)` | **exact** | `traits.rs:1859-1870` |
| `embed_entity` | `set_field(content, old)` | **exact** | restores pre-embed content — `traits.rs:1816-1825` |
| `insert_text` | `delete_text(pos, len)` | **exact** *(NEW)* | scalar-count inverse — `loro_block_operations.rs` (this change) |
| `delete_text` | `insert_text(pos, captured)` | **exact** *(NEW)* | captures deleted substring — `loro_block_operations.rs` (this change) |
| `apply_mark` | — | **absent** | irreversible — `loro_block_operations.rs:912` |
| `remove_mark` | — | **absent** | irreversible — `loro_block_operations.rs:946` |
| `cycle_task_state` | `set_field(task_state, old)` | **exact** | delegates set_state→set_field — `:848-861` |
| `set_state` | `set_field(task_state, old)` | **exact** | `:835-846` |
| `set_title` | `set_field(content, old)` | **exact** | `:798-810` |
| `set_due_date` | — | **absent** | writes `DEADLINE` property → set_field `_` arm — `:863-875,449` |
| `set_priority` | — | **absent** | writes `PRIORITY` property → set_field `_` arm — `:877-880,449` |
| `add_tag` | `remove_tag` | **exact** | element-wise, idempotent — `loro_block_operations.rs:216-249` |
| `remove_tag` | `add_tag` | **exact** | symmetric — `:283-310` |
| `dismiss_advice` | — | **absent** | append-only suppression, irreversible — `:161` |
| `rewrite_link_resolution` | `restore_link_resolution(captured rows)` | **exact** *(NEW)* | junction inverse for block→page inbound re-point; captures prior `(source_block_id, target, kind, resolved_id)` PK tuples, restores each (capture-based, NOT a `to→from` swap) — `sql_operation_provider.rs` (this change) |

### Coverage stats (post-implementation)

- **N = 25 dispatched operations** (`set_field` and `delete` are field/mode-
  sensitive — counted once as an op, with the sub-cases spelled out above;
  `rewrite_link_resolution` is the new junction surface. Its internal inverse
  twin `restore_link_resolution` is inverse-only and not counted.)
- **Exact:** 16 ops unconditionally exact — `move_block`, `indent`, `outdent`,
  `move_up`, `move_down`, `embed_entity`, `insert_text`, `delete_text`,
  `cycle_task_state`, `set_state`, `set_title`, `add_tag`, `remove_tag`,
  `rewrite_link_resolution` (NEW), plus `set_field` on its covered fields
  (content-String / **content-Object+marks (NEW)** / **marks (NEW)** /
  task_state / edge).
- **Conditional (M, exact-in-common-case, fail-loud otherwise):** 5 —
  `create`, `delete` (SqlOnly), `update`, `split_block`, `join_block`.
- **Absent (J):** 5 ops — `apply_mark`, `remove_mark`, `set_due_date`,
  `set_priority`, `dismiss_advice` — **plus** ONE remaining systemic sub-gap:
  `delete` under the **default Loro authority**. The prior
  `set_field(marks / rich-content)` sub-gap is now CLOSED (this change); the
  arbitrary-property sub-gap remains (folded into `set_due_date`/`set_priority`
  above).

Before this change `insert_text`/`delete_text` were absent (closed earlier);
`set_field(marks / Object content)` and the junction rewrite were absent — this
change closes both (Option-B's two blockers).

## Block→page transform — gate verdict

The transform is acceptable as **one composite `UndoEntry` only if all its
constituent ops have inverse providers** (transform doc Q5). Mapping each option
to the table:

| Option | Constituent ops | Undo-gate |
|---|---|---|
| **A** — in-place retag | `add_tag("Page")` (+ ancestor `add_tag`s) [+ `move_block` if re-anchored] | **PASSES** — every op is exact (`add_tag`↔`remove_tag`, `move_block` exact). De-inlining is a write-back side effect, not an op. |
| **B** — page + link | `create` (page) + N×`move_block` (children) + rewrite origin content to `[[P]]` + backlink `resolved_id` rewrite | **CLEARED** *(NEW)* — `create`/`move_block` exact; the origin-becomes-link marked-content write now has an exact `set_field(Object/marks)` inverse; and the `block_links` junction rewrite now has an operation-level inverse (`rewrite_link_resolution` ↔ `restore_link_resolution`). Every constituent op is invertible ⇒ one composite `UndoEntry`. |
| **C** — page + move + delete | `create` + N×`move_block` + `delete` origin | **BLOCKED** — `delete` under the **default Loro authority is absent**. Confirms the transform doc's "delete is irreversible under today's providers." |

**Verdict:** With this change, **Options A and B both clear the composite-undo
gate**; only **C** remains gated — on a Loro-authority `delete` inverse (the
last systemic sub-gap, shortlist #1). If the block→page ruling selects B (the
LogSeq-style recommended default), it is now undoable as one entry. (Option A
being the undo-cleanest still dovetails with the undo ruling's "A shaped for C"
framing.)

## Ranked shortlist — which missing providers to build first

Ranked by *user-facing frequency first, then transform-prerequisite weight*.

1. **`delete` under Loro authority** *(highest — default prod mode, common user
   action, and Option-C prerequisite).* Port the SqlOnly leaf-capture pattern
   (`sql_operation_provider.rs:1885-1940`): capture full row + edges before
   delete → `create` inverse; cascade stays `DeclaredIrreversible`. Larger than
   a quick-win (needs Loro-side subtree/edge capture) → **shortlist**.

2. ~~**`set_field(marks)` / `set_field(Object content)`**~~ **DONE (this
   change).** Captures prior `(text, marks)` and restores both atomically via a
   `content=Object` inverse routed through `update_block_marked` (whole-set
   mark replace) — so an originally-plain block genuinely restores plain rather
   than leaving Peritext marks pinned to surviving scalars.

3. **`apply_mark` / `remove_mark`** *(high — incremental toolbar mark ops).*
   `apply_mark`'s inverse cannot be a blind `remove_mark` (would strip
   pre-existing overlapping marks) — needs a per-range prior-mark capture.
   Medium-hard; do after #2 (shares the mark-capture machinery).

4. **`set_field(arbitrary property)`** *(medium — `set_due_date`, `set_priority`,
   any generic property).* Generalize the `task_state` arm
   (`loro_block_operations.rs:418-431`): capture prior property value → inverse
   `set_field(field, old)`. Easy-medium; a near-mechanical extension.

5. ~~**Junction / backlink `resolved_id` rewrite inverse**~~ **DONE (this
   change).** Introduced the operation-level surface `rewrite_link_resolution`
   `{from, to}` (re-point every `block_links` row resolved to `from` onto `to`)
   with an exact capture-based inverse `restore_link_resolution` that restores
   each affected PK row's prior `resolved_id`. Scoped to the block→page
   transform's inbound re-point; NOT a generic SQL-undo framework.
   **`dismiss_advice`** (append-only suppression) is intentionally *lowest*
   priority — low undo value.

## What this change landed

- `insert_text` and `delete_text` in `LoroBlockOperations` return **exact**
  inverses (earlier change): `insert_text`→`delete_text(pos, scalar_count)`,
  `delete_text`→`insert_text(pos, captured_substring)`.
- **`set_field(content=Object{marks})` and `set_field(marks)`** in
  `LoroBlockOperations` now return **exact** inverses (previously
  `irreversible`). Prior `(content, marks)` is captured up front (already read
  for the fingerprint) and the inverse is a single
  `set_field(content, Object{prior text, prior marks})` — routed through
  `update_block_marked`, which unmarks every key over the full range before
  re-applying, so text AND marks restore atomically and an originally-plain
  block comes back plain. The `content` stale-guard is armed with a real
  `FieldDelta` only when the text changed (a marks-only edit reports empty
  `changes`, single-writer-safe, so the engine's vacuous-write filter does not
  drop the entry). Unit tests: `set_field_object_content_is_reversible_text_and_marks`,
  `set_field_marks_only_is_reversible`, `set_field_object_content_multibyte_roundtrip`.
- **`rewrite_link_resolution` / `restore_link_resolution`** in
  `SqlOperationProvider` — the junction inverse Option B needs. The forward op
  re-points every `block_links` row resolved to `from` onto `to`; the inverse
  captures the affected rows' prior `(source_block_id, target, kind,
  resolved_id)` PK tuples and restores each exactly (capture-based, so a row
  that already resolved to `to` before the rewrite is left untouched by undo).
  Both descriptors are registered so the dispatcher routes them. Unit tests:
  `rewrite_and_undo_restores_prior_resolved_ids`,
  `undo_does_not_touch_rows_preexisting_at_target`.
- Test-harness fix: `delete_inverse_classification_tests::provider_with_rows`
  now creates `block_links` (via the canonical `LinkSchemaModule`, plus a
  `content_type` column the `backlinks` matview requires) — the `delete`
  cascade's `block_links` cleanup previously failed with "no such table:
  block_links", so both delete-inverse tests were red pre-existing.

### DeclaredIrreversible sub-cases (none)

Both providers are exact for every sub-case in scope. `restore_link_resolution`
returns `DeclaredIrreversible` by design — it is an inverse-only surface (redo
re-runs the forward `rewrite_link_resolution`), and its classification is
ignored on inverse replay. No lossy fallbacks were introduced.
</content>
</invoke>
