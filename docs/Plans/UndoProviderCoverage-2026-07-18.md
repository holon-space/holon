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
| `set_field(content=Object{marks})` | — | **absent** | rich/marked content stays irreversible — `loro_block_operations.rs:390` |
| `set_field(marks)` | — | **absent** | mark-only edit irreversible — `loro_block_operations.rs:390,449` |
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

### Coverage stats (post-implementation)

- **N = 24 dispatched operations** (`set_field` and `delete` are field/mode-
  sensitive — counted once as an op, with the sub-cases spelled out above).
- **Exact:** 14 ops unconditionally exact — `move_block`, `indent`, `outdent`,
  `move_up`, `move_down`, `embed_entity`, `insert_text` (NEW), `delete_text`
  (NEW), `cycle_task_state`, `set_state`, `set_title`, `add_tag`, `remove_tag`,
  plus `set_field` on its covered fields (content-String / task_state / edge).
- **Conditional (M, exact-in-common-case, fail-loud otherwise):** 5 —
  `create`, `delete` (SqlOnly), `update`, `split_block`, `join_block`.
- **Absent (J):** 5 ops — `apply_mark`, `remove_mark`, `set_due_date`,
  `set_priority`, `dismiss_advice` — **plus** the two systemic sub-gaps:
  `delete` under the **default Loro authority**, and `set_field` on
  **marks / rich-content / arbitrary properties**.

Before this change `insert_text`/`delete_text` were also absent; they are now
exact (2 gaps closed).

## Block→page transform — gate verdict

The transform is acceptable as **one composite `UndoEntry` only if all its
constituent ops have inverse providers** (transform doc Q5). Mapping each option
to the table:

| Option | Constituent ops | Undo-gate |
|---|---|---|
| **A** — in-place retag | `add_tag("Page")` (+ ancestor `add_tag`s) [+ `move_block` if re-anchored] | **PASSES** — every op is exact (`add_tag`↔`remove_tag`, `move_block` exact). De-inlining is a write-back side effect, not an op. |
| **B** — page + link | `create` (page) + N×`move_block` (children) + rewrite origin content to `[[P]]` + backlink `resolved_id` rewrite | **BLOCKED** — `create`/`move_block` are exact, but the origin-becomes-link write is **marked content** (a `Link` Peritext mark) → `set_field(Object/marks)` is **absent**; and the `block_links` junction rewrite has no operation-level inverse (no FK cascade — transform doc §backlinks). |
| **C** — page + move + delete | `create` + N×`move_block` + `delete` origin | **BLOCKED** — `delete` under the **default Loro authority is absent**. Confirms the transform doc's "delete is irreversible under today's providers." |

**Verdict:** With today's providers, **only Option A clears the composite-undo
gate.** This is a genuine input to the still-pending block→page ruling: if the
ruling wants B (the LogSeq-style recommended default) or C to be undoable as one
entry, it is **gated on building the missing providers below** — B on the
marks/rich-content content inverse **and** a junction-rewrite inverse; C on a
Loro `delete` inverse. (Option A being the undo-cleanest dovetails with the undo
ruling's "A shaped for C" framing.)

## Ranked shortlist — which missing providers to build first

Ranked by *user-facing frequency first, then transform-prerequisite weight*.

1. **`delete` under Loro authority** *(highest — default prod mode, common user
   action, and Option-C prerequisite).* Port the SqlOnly leaf-capture pattern
   (`sql_operation_provider.rs:1885-1940`): capture full row + edges before
   delete → `create` inverse; cascade stays `DeclaredIrreversible`. Larger than
   a quick-win (needs Loro-side subtree/edge capture) → **shortlist**.

2. **`set_field(marks)` / `set_field(Object content)`** *(high — every bold /
   italic / link / rich-text edit is irreversible today, and it is Option-B's
   blocker).* Capture prior `(text, marks)` → inverse restores both. Medium.

3. **`apply_mark` / `remove_mark`** *(high — incremental toolbar mark ops).*
   `apply_mark`'s inverse cannot be a blind `remove_mark` (would strip
   pre-existing overlapping marks) — needs a per-range prior-mark capture.
   Medium-hard; do after #2 (shares the mark-capture machinery).

4. **`set_field(arbitrary property)`** *(medium — `set_due_date`, `set_priority`,
   any generic property).* Generalize the `task_state` arm
   (`loro_block_operations.rs:418-431`): capture prior property value → inverse
   `set_field(field, old)`. Easy-medium; a near-mechanical extension.

5. **Junction / backlink `resolved_id` rewrite inverse** *(medium — strictly an
   Option-B prerequisite, no standalone user op today).* Only build if the
   block→page ruling selects B. **`dismiss_advice`** (append-only suppression)
   is intentionally *lowest* priority — low undo value.

## What this change landed

- `insert_text` and `delete_text` in `LoroBlockOperations` now return **exact**
  inverses (previously `irreversible`): `insert_text`→`delete_text(pos,
  scalar_count)`, `delete_text`→`insert_text(pos, captured_substring)`. Both arm
  the `content` stale-guard with a real `FieldDelta`. Unit tests
  (`insert_text_is_reversible_with_exact_delete_inverse`,
  `delete_text_is_reversible_with_exact_insert_inverse`) assert the inverse shape
  **and** round-trip the block content byte-for-byte.
</content>
</invoke>
