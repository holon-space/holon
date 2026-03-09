# Handoff — Phase 2 trait shape + Loro architectural roles

Continuation of `devlog/2026-05-09-002223-phase-2-design-discussion-handoff.md`.
Closes the day with two locked-in decisions for the trait, and a wider
architectural discussion captured for the next session.

## Where we are

Stack unchanged from the previous handoff. No new commits in this
session — discussion only.

```
@  rokmplpx (empty)                — placeholder for next step
○  vwzowtxv 56495cf9               — Phase 2 step 3: drop _expected_content watermark
○  xtytokul b0252de4               — Phase 2 step 2: route split/join/embed_entity content via Loro
○  zpxvpplt 2a8d3999               — Phase 2 step 1: handle_text_sync no-op for content
○  qownouvo 2e481cc4               — Phase 0+1: BlockContentResolver + headless mirror + reproducer
○  uqtvotzp 48cee3d6                — fix: PBT panics + Turso IVM bug capture (parent of branch)
```

Recipe stays GREEN. The deterministic `bulk-1-0 = "LM"` flake is
pre-existing and tracked separately.

## Decisions the user locked in this session

1. **Rename `BlockContentResolver`.** The trait should be named for what
   it actually does: per-field text merge / live source. Working names:
   `TextMergeStore` or `LiveTextSource`. Final name TBD; either captures
   the fact that the scope is one text-shaped field, not the whole block
   tree. The current name suggests a wider scope than the trait actually
   covers, which makes reasoning fuzzier.

2. **Parameterize on `(EntityUri, field)`** rather than `block_id`. Free
   generalization for the day a JIRA-issue description, Todoist task
   content, or any other text-mergeable field on a non-block entity wants
   the same per-field CRDT-merge mechanism. No cost today (block IDs are
   already EntityUris); makes the trait re-usable later without a
   second rename.

The user explicitly noted (correctly) that decisions 1+2 are about
naming and parameter shape — they do **not** commit to any particular
view on Loro's overall role in the architecture. They sharpen the
trait to be honest about its current scope.

## Decisions still on the table for the trait itself

Carried over from the previous handoff with the user's provisional
answers:

- **`OperationResult.changes` contract** — provisional answer (A): chord
  ops always return no synchronous `FieldDelta` for content; the SQL
  UPDATE arrives downstream via `on_loro_changed` → CDC. Audit needed of
  any caller that today reads `result.changes` looking for content
  updates. Most concrete consumer:
  `crates/holon/src/core/operation_wrapper.rs:98`
  (`sync_provider.sync_changes(&result.changes)`).
- **Routing site** — provisional answer: keep `set_live_content` as the
  chord-op-facing API on `BlockOperations`. The leading-store routing
  hides inside `SqlBlockOperations::set_live_content`. Chord ops call
  one method (`self.set_live_content(uri, "content", value)`); they
  never see the underlying store. The `Ok(bool)` return + chord-op-side
  fallback ladder go away. Test paths without a wired store fall back
  to `set_field` *inside* the impl.
- **Migration plan** — depends on the architectural direction (next
  section). If the answer is "land the trait rename + Turso impl now,"
  the cleanest move is to replay step 2 + 3 on top of the renamed
  trait. If the answer is "we want to revisit the whole mid-architecture
  first," step 2/3 should freeze in their current shape until that
  settles.

## The wider architectural discussion

The discussion zoomed out from "what trait shape" to "what is Loro
fundamentally doing here, and is that the architecture we want?"

### Refined role decomposition (4 roles, not 3)

The user originally framed this as 3 roles with the third partially
orthogonal. Splitting role 3 makes the picture sharper:

- **Role A — CRDT merge semantics for text.** RGA + Peritext for
  character-level concurrent edits. LWW would lose concurrent-edit
  data, so this is the role where Loro is genuinely irreducible.
- **Role B — CRDT-aware structural moves.** LoroTree's
  moveable-tree-with-cycle-prevention under concurrent edits (Kleppmann
  move algorithm). Optional in single-user mode; required if peers
  concurrently indent/move the same block.
- **Role C — Persistent storage of the block tree** (just persistence).
- **Role D₁ — Wire format for sync transport** (Loro snapshots are
  compact binary diffs).
- **Role D₂ — Merge-at-receive-time** (= an instance of role A applied
  to whatever just arrived).

Plus two roles surfaced in passing:

- **Role E — Causal history** (vector-clock-based "what did peer X do?"
  / `fork_at(&watermark)` for outbound diffs).
- **Role F — Awareness/cursors/presence** (not used today).

The user's framing remark: Loro multi-role isn't *bad* — they like
"all the things it gives us for free, it almost seems made for this
collaborative outliner use case." The cost is that **reasoning about a
multi-role component is harder than reasoning about a single-role
one**, which is what made the trait-shape discussion feel entangled.
That's a reasoning-cost observation, not an indictment.

### Three architectural endpoints (the user's framing)

The discussion surfaced three coherent endpoints. Pros/cons listed as
neutrally as possible — the user is not committed to any of them yet.

#### (α) Status quo — Loro as additional source of truth in Full mode

What we have today plus Phase 2 sharpening of the read/write boundary
for content.

**Pros**
- Already implemented and largely working.
- SQL keeps its strengths (matview CDC, JOINs, recursive CTEs, FK
  constraints) for structure + LWW fields.
- Loro keeps its strengths (CRDT text merge, moveable tree) for the
  small set of fields that benefit.
- SqlOnly mode is automatic — no Loro wired, no Loro path taken.
- External writers (org parser / MCP) keep simple `set_field` semantics.

**Cons**
- Two-system reasoning. "Where does this field live?" is a real
  question with a context-dependent answer.
- `LoroSyncController` is large. Watermark management, fork-at-watermark
  for outbound diff, sidecar persistence, echo suppression, special-case
  `apply_fields_changed` per field type.
- Most PBT pain has been at the SQL/Loro boundary rather than within
  either system in isolation.

#### (β′) Loro / Org / Markdown as peer "primary" storage; Turso always the projection

The user's revisited (β). **Refined version** of the original "Loro as
OperationProvider": any of `{Loro, Org files, Markdown files}` can be
configured as the *primary* writer for blocks; the others (and Turso)
subscribe via the EventBus and project. Turso is always present as the
read/query layer — but never directly written to by a chord op.

**Pros**
- Architectural unification — every storage system is a peer. Loro
  isn't special; neither is Turso.
- Configurable — Holon-without-Loro becomes a valid mode (primary =
  Org; reads via Turso projection). Holon-with-only-text-files is a
  coherent product (text-files-first PKM in the LogSeq / Org-roam
  family).
- The trait surface is well-defined: implement `OperationProvider` for
  blocks. Higher-level ops (split, join, move) decompose to atomic ops
  via the existing trait defaults — that decomposition is reused.
- Symmetric with how external systems (Todoist, JIRA, GMail) already
  work conceptually, just for a non-overlapping data shape.

**Cons**
- Implementation cost. `LoroOperationProvider`, `OrgOperationProvider`
  don't exist; each is a non-trivial adapter.
- Read-on-primary is awkward. Chord-op decomposition reads block state
  to compute (e.g. `split_block` reads content, `get_next_sibling`,
  `get_children`). If primary = Org, those reads are slow without a
  Turso-style projection. Likely shape: writes to primary, reads from
  Turso (so Turso is on the read path even when not on the write path).
- Inter-primary consistency. If two primaries are *both* enabled (Loro
  + Org), who wins on conflict? Probably the answer is "exactly one
  primary at a time; others are bidirectional subscribers" — which
  reduces the symmetry but keeps the model coherent.
- `set_field`-style writes from the primary still need to propagate to
  the others. For Loro-as-primary that's the existing
  `on_loro_changed` projector. For Org-as-primary that's a new diff +
  EventBus emit step. New code to write per primary.

#### (γ) Loro reduced to role A only

Make Loro a *merge oracle*, not a storage system. SQL owns block tree
structure (B in single-user mode is replaced by LWW; B as a true CRDT
move semantic is dropped). No global LoroDoc; per-field, per-session
LoroText instances spun up at editor open and serialized to bytes on
commit (or kept ephemeral). Sync transport (D₁) is whatever the
EventBus chooses (Org files, JSON, network); D₂ at peer reconnect uses
a fresh LoroText constructed from the persisted state to merge.

**Pros**
- Smallest Loro surface area. Most of `LoroSyncController` and the
  watermark/sidecar machinery becomes deletable.
- Generalizes naturally to non-block fields (JIRA-description offline
  merge, Todoist task content) — the trait is per-field.
- Eliminates the SQL/Loro storage-coordination bug class.

**Cons**
- Loses role B (concurrent moveable-tree CRDT). LWW on `parent_id` /
  `sort_key` for concurrent peer moves; for most realistic team
  workflows this is fine, but it is a loss.
- Loses role E at the system level (causal history, peer audit trail).
- Loses Loro snapshots as a transport optimization (D₁) — wire format
  becomes EventBus payloads or whatever each transport chooses.
- Loro is "made for this use case"; (γ) intentionally throws away the
  use of Loro that pulls in the most of its surface area. The user's
  resistance to this trade is reasonable.

### The central question raised by (β′)

> Is there a common set of operations that Loro, Org and Markdown
> support, which is not much / complex code so we can afford to
> duplicate it and express more complex operations like split-block,
> join-block, move-block in terms of it. And how does that relate to the
> event bus? Or does it exactly correspond to the events from the event
> bus we need to handle anyway?

**My read** (offered as input, not a push):

The minimal common op set on a primary is approximately:

- `create(entity_name, fields)`
- `delete(entity_name, id)`
- `set_field(entity_name, id, field, value)` (covers move via
  `parent_id` / `sort_key`, completion via `completed`, content via
  `content`, etc.)

That's three primitives. They map almost 1-to-1 to the existing
EventKind set:

- `EventKind::Created` ↔ `create`
- `EventKind::Deleted` ↔ `delete`
- `EventKind::FieldsChanged` ↔ `set_field`

So yes, the minimal common op set ≈ the EventKind set. Each primary's
write path is "apply this primitive locally, emit the corresponding
event"; subscribers apply the inverse.

**Higher-level ops decompose to the primitives.** This is already true
in the codebase: `BlockOperations` provides default implementations of
`split_block` / `join_block` / `indent` / `outdent` / `move_block` /
`embed_entity` that decompose to `create` / `delete` / `set_field`
calls on `self`. If a primary implements just the three primitives,
the higher-level ops come for free via the trait defaults. (Today
`SqlBlockOperations` is the only concrete primary; the defaults are
exercised through it.)

**Implications**
- Per-primary code is small — three methods, plus the primary's own
  emit/subscribe loop for the EventBus. That makes (β′) more tractable
  than it sounds.
- The decomposition is shared, so a `SplitBlock` on Loro-as-primary and
  on Org-as-primary do "the same SQL-ish things" expressed against
  different write backends. The decomposition itself doesn't have to
  know about the backend.
- Reads still need Turso (matviews, JOINs, recursive descendant
  queries). Even in (β′) Turso isn't optional; it's just demoted from
  "writer" to "reader/query engine." That matches the user's framing
  ("Turso is a pure event bus subscriber and we basically never perform
  direct data modifications on it").

**Caveats**
- The `set_field` primitive is uniform on the API but heterogeneous
  underneath. For Loro it's `tree.meta.set` or `LoroText.update`. For
  Org it's "rewrite the relevant region of the file" — coarse-grained
  unless we do incremental file edits. That's where the implementation
  cost concentrates per primary.
- `delete` for Org/Markdown is "remove a region from a file"; for Loro
  it's `tree.delete`. Same shape, different mechanics.
- Field-level CRDT merge (role A) is a *separate* concern from the
  primary-writer role. Even in (β′), if you want concurrent text
  merge, you still need Loro (or a Loro-like CRDT) for role A. (β′)
  doesn't replace (γ); they're orthogonal.

So: **(β′) and (γ) can coexist.** You could have Org-as-primary for
storage and Loro-as-merge-oracle for role A. Or Loro-as-primary
(covering A + storage). Or Turso-as-primary (status quo with rename).
The trait shape we land in Phase 2 is mostly independent of which
endpoint you eventually pick.

### Why the user's complexity framing matters

The user's clarification — "I didn't mean to say that Loro takes too
many responsibilities, it just makes thinking about it more
entangled/complex than thinking about a component that only does a
single thing" — is the right diagnosis. The decisions 1+2 above
(rename + parameterize) are exactly the sort of low-cost moves that
**reduce reasoning entanglement without committing to a particular
architecture**. They make the trait honest about its scope; whether
that scope eventually grows (β′) or shrinks (γ) is a separate
discussion.

## What Phase 2 should do regardless of the architectural direction

Independent of which endpoint wins later, the following are net wins
that the user has already agreed to:

1. Rename the trait (`BlockContentResolver` → e.g. `TextMergeStore` /
   `LiveTextSource`).
2. Parameterize on `(EntityUri, field)` rather than `block_id`.
3. Collapse `set_live_content` into a single method on
   `BlockOperations` with `Result<()>` return; routing hides inside
   `SqlBlockOperations::set_live_content`. Drop the chord-op-side
   `if !set_live_content { set_field }` ladder.
4. Add a `TursoTextMergeStore` (or whatever the name lands as) so
   SqlOnly mode also has a wired store rather than a `None` fallback.
   This makes the "fallback to SQL via set_field" path used only by
   the synthetic in-memory test store.

These four are forward-compatible with (α), (β′), and (γ). They don't
prejudge the larger question.

## Question to settle next session before any code

- Are decisions 1–4 above the right set to start coding from? (Likely
  yes, given the user's stated agreement on 1+2 and the previous
  session's provisional agreement on 3+4.)
- If yes, name finalization: `TextMergeStore` vs `LiveTextSource` vs
  something else. The user offered both; either works. The name has
  to read naturally at the chord-op call site
  (`self.text_merge_store(uri, "content")` vs `self.live_text(uri,
  "content")` etc.).
- Is the architectural conversation (α / β′ / γ) something to leave
  parked, or does the user want to spike one direction (e.g., a
  prototype `LoroOperationProvider` for blocks to validate (β′)) before
  Phase 2 lands? My read of the user's last message is "park; finish
  Phase 2 cleanly first; revisit architecture as a separate
  conversation."

## Files and call sites for the Phase 2 work

Same as the previous handoff. Reproduced for convenience:

- `crates/holon-core/src/traits.rs:498-558` — `BlockContentResolver`
  trait + `BlockOperations::live_content` /
  `BlockOperations::set_live_content`. This is the rename + parameter
  refactor target.
- `crates/holon/src/sync/loro_block_content_resolver.rs` — the Loro
  impl. Becomes `LoroTextMergeStore` (or similar).
- `crates/holon/src/core/sql_block_operations.rs:39-100` —
  `SqlBlockOperations`, with `content_resolver: Option<Arc<dyn
  BlockContentResolver>>`. After Phase 2 the field becomes non-optional
  (every mode wires a store; the in-memory test store keeps a
  `NoopTextMergeStore` or just stays on the trait default).
- `crates/holon-core/src/traits.rs:765-883` — `split_block` callsite.
  Drops the `if !set_live_content { set_field }` ladder.
- `crates/holon-core/src/traits.rs:909-1030` — `join_block` callsite.
  Same change.
- `crates/holon-core/src/traits.rs:1078-1120` — `embed_entity`
  callsite. Same change.
- `crates/holon/src/sync/event_infra_module.rs:103-132` — DI wiring.
  Currently does `optional_resolve_async::<dyn BlockContentResolver>`
  and threads via `with_content_resolver`. After Phase 2, both Loro
  and Turso modules register an impl, and SqlOnly mode resolves to
  `TursoTextMergeStore`.
- `crates/holon/src/core/operation_wrapper.rs:98` — the most concrete
  consumer of `result.changes` for cross-system sync. Needs a quote
  in the `OperationResult.changes` contract decision (no synchronous
  FieldDelta for content writes that go through Loro).
- `crates/holon/src/sync/loro_sync_controller.rs:259-389` — `on_inbound_event`
  + `on_loro_changed`. Stays. The user verified that
  `apply_fields_changed` keeps Loro current for non-Loro-origin events,
  so inbound SQL→Loro stays for org parser / MCP writes regardless.

## What didn't get said (verbatim quotes worth preserving)

The user's words on the architectural question, from the last two
turns, that should survive context wipe:

> What if we treat Loro like other external datasources like Todoist,
> JIRA, GMail, ... That would imply that Loro is the
> `OperationProvider` for everything block-related, block-splitting,
> joining, text editing, ...

> In principle one could then even operate Holon without Loro &
> blocks, given that we allow storing queries (not yet possible) and
> entity profiles (already possible in YAML files) outside of blocks.
> Holon configured without Loro would then be just a system that
> displays and interoperates with external systems without any
> separate storage.

> What happens if we edit data from external systems like a JIRA
> issue? The way we have wired this up so far would go through
> `MutableText` (and with it Loro). That could be an advantage if
> multiple people change the description of a single JIRA issue (e.g.
> one in the external system, and one through Holon, which becomes
> much more probable once we allow offline usage). The offline use
> case for an external system is actually quite interesting. If we
> really want to do proper text merging, we actually need something
> like Loro.

> Loro fulfills multiple (3?) purposes. 1. For merging text in a
> semantically meaningful way. Could be replaced by LWW. 2. As storage
> for our block tree structure. Could be replaced with Turso or
> Org/Markdown. 3. As sync mechanism in combination with Iroh. Could
> be replaced by Org/Markdown + file sync.

> I didn't mean to say that Loro takes too many responsibilities, it
> just makes thinking about it more entangled/complex than thinking
> about a component that only does a single thing. I actually like
> all the things it gives us for free, it almost seems made for this
> collaborative outliner use case with the collaborative tree
> structure and text.

> I also don't find β so bad … Handle Loro / Org / Markdown like an
> external (storage) system and you can leave any one of them out,
> but the ones that are on board do constantly sync via event bus.
> And Turso is a pure event bus subscriber and we basically never
> perform direct data modifications on it, only observe modifications
> through Loro / Org / Markdown.

> Is there a common set of operations that Loro, Org and Markdown
> support, which is not much / complex code so we can afford to
> duplicate it and express more complex operations like split-block,
> join-block, move-block in terms of it. And how does that relate to
> the event bus? Or does it exactly correspond to the events from the
> event bus we need to handle anyway?

## Open random-seed flake (do not block on) — RESOLVED 2026-05-09

`bulk-1-0 = "LM"` (prod) vs `"LM lX8G"` (ref) deterministic divergence
on a specific seed. Pre-existing; unrelated to Phase 2. Tracked
separately in `devlog/2026-05-08-220908-…`.

**Resolved 2026-05-09**: ref-model bug, not prod. The actual seed shrank
to `BulkExternalAdd → FocusEditableText(bulk-1-0) → PinBlock →
DeleteBackward(count=4)`. After 4 backspaces from `"LM lX8G"`, prod's
`LoroText = "LM "` (trailing space) → `SqlOperationProvider::trimmed_content`
trims to `"LM"`. Ref's `DeleteBackward::apply_to_ref` lacked the
`commit_active_editor_if_changed()` call that `TypeChars::apply_to_ref`
uses when `enable_loro` (its `// see TypeChars apply_to_ref for
rationale` comment was stale), and `commit_active_editor_if_changed`
itself didn't apply trim normalization. Two surgical fixes:
`DeleteBackward::apply_to_ref` now commits when `enable_loro`, and
`commit_active_editor_if_changed` runs `in_memory_content` through
`normalize_content_for_org_roundtrip` before writing `block.content`.
Both PBT variants PASS post-fix (Full 452s, SqlOnly 487s). See
`devlog/2026-05-09-typechars-divergence-fixed.md` and MEMORY entry
`pbt_deletebackward_ref_commit_trim.md`. The earlier hypothesis list
(Weak-keyed cache eviction races, async-vs-sync ordering, `text.update`
truncation) was unnecessary — the bug never reached the Loro layer.

---

## CLOSING ADDENDUM (2026-05-09 — Phase 2 LANDED)

The trait/roles questions resolved by the cells plan
(`~/.claude/plans/ok-i-think-we-snappy-pnueli.md`) and Phase 2 of its
execution: `Cell<T>` is the universal reactive primitive,
`BlockCellRegistry::write_field` is the dispatcher, Loro is sole
writer for SQL block columns. The `bulk-1-0` TypeChars divergence
called out here as "do not block on" was investigated and fixed in a
follow-up session — see the "Open random-seed flake" addendum above
and `devlog/2026-05-09-typechars-divergence-fixed.md`. See also
`devlog/2026-05-09-175751-phase2-authority-flip-landed.md` and MEMORY
entry `cells_and_loro_authority.md`.
