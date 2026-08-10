# Arm (d)-narrow — estimation spike (read-only; the 14-file diff is untouched)

**Headline: lane-sized, not a re-architecture — 2–4 focused days.** The seams it
needs are already singular and already documented as the intended future shape.
It is a NET DELETION, it fixes two defects this lane could only file, and it is
strictly less work than (b′) because (b′) does (d)'s hard half (raw seeding) and
then keeps the machinery (d) deletes.

---

## 0. The finding that decides the size

`EditorViewModel::set_buffer_from_authority` already carries this doc comment,
written before this lane:

> RAW-seam hook: `text` MAY be a raw reconstruction (`render_inline_marks`) once
> raw-edit mode lands; today callers pass stored (stripped) content and **the
> signature does not change when raw seeding arrives.**

And `apply_local_edit`:

> RAW-seam hook: the `set_field` intent path is the single point where the
> dispatcher re-extracts inline marks on commit; **keep it the sole commit
> funnel so raw→stripped extraction has exactly one home.**

(d)-narrow IS that seam, restricted to the task-keyword facet instead of marks.
The architecture was designed for it; nothing has to be invented. The
mark-flavoured twin (`render_inline_marks` ⇄ `extract_inline_marks`) is already
proptest-proven as a round trip, so the pattern has a working precedent in the
same crate.

---

## 1. SIZE — concrete touch points

### Production (small, and each is a SINGLE site)

| # | Touch point | Work |
|---|---|---|
| 1 | `source_projection(task_state, content) -> String` in `holon-org-format` | NEW, ~10 lines; the inverse of `keyword_headed`. Its round-trip proptest is the natural red-first rung. |
| 2 | `EditorView::converge_input` (`frontends/gpui/src/views/editor_view.rs:739`) — **the single convergence entry point** | `target` becomes the source projection. Needs `task_state` alongside the text; the row projection already carries it (that is exactly what #64's `TaskKeywordAtKeystroke::Read` reads in GPUI). ~15 lines. |
| 3 | `HeadlessEditorMirror::converge_editor` (`headless_editor_mirror.rs:283`) | same change, headless twin; `sql_block_content` gains the `task_state` column. ~10 lines. |
| 4 | `EditorViewModel::apply_local_edit` | **DELETE** the `promote_task_keyword` branch. The commit becomes a plain `set_field(content, raw_buffer)` — the store's eager convergence (already landed in this diff) IS the parse. No new op, no new dispatch. Net −80 lines. |
| 5 | Caret mapping on focus seed | The real work. See §3. ~30 lines + tests. |
| 6 | Deletions | `candidate_keyword_headed`, `candidate_promotion`, `TaskKeywordAtKeystroke`, `EditorViewModel::promote_task_keyword`, `restore_refused_promotion`, `PROMOTE_TASK_KEYWORD_OP` + `run_promote_task_keyword`, `PromotionRefusal`, `dispatch_task_keyword_constituent`'s promotion arm, `could_converge`'s editor twin. Roughly **−600 lines**. |

Unfocused display is untouched: the pill is a separate `state_toggle` widget
(`focus_path.rs`), and `render/builders/editable_text.rs` has **no** `task_state`
reference today — it only gains the seed input.

### Tests

| File | Verdict |
|---|---|
| `frontends/gpui/tests/editor_task_keyword_promotion.rs` (10 tests) | **6 DELETE** — `typing_the_keyword_promotes_and_clears_it_from_the_visible_field`, `the_caret_follows_the_stripped_text`, `a_bare_keyword_is_left_alone`, `a_second_keyword_after_a_promotion_is_ordinary_text`, `a_refused_promotion_puts_the_keyword_back` (+ its cell twin): all test strip/refusal machinery that ceases to exist. **4 REWRITE** — the mount/arrival/cell-visibility trio becomes "focus seeds the source projection" (`an_editor_mounted_on_an_existing_task_does_not_promote` → `…seeds_the_keyword_into_the_buffer`). |
| `frontends/gpui/tests/live_promotion_windowed.rs` (1 test) | **REWRITE**, and it gets *simpler*: focused row shows `TODO milk`; Cmd-Z walks the text back; unfocused paints pill + `milk`. |
| `crates/holon/tests/promote_task_keyword_compound.rs` (24 tests) | ~**14 DELETE** (the compound is gone), ~10 MIGRATE to store-level convergence tests they already nearly are. |
| Keystone ref `pbt/transitions/type_chars.rs` | **SIMPLIFIES to nothing special**: delete the promotion branch entirely and let `commit_active_editor_if_changed` commit raw text; the ref's *store* applies the convergence rule. This is the answer to "does ref simplify to parse-of-typed-text?" — **yes**, and that is the strongest smell test that (d) is the right shape. |
| `reference_state.rs` | editor-mirror seeding must render the source projection on `FocusEditableText`. ~15 lines. |

### Undo — the #64 Inc 4b contract

It **survives and gets better**. Under (d) the promoting keystroke is an ordinary
content commit; the store converges it, and the composite undo entry my diff
already builds (`converge_forwards` / `converge_inverses`) is exactly "one press
restores both the text and the absent task state". Inc 4b's guarantee is
preserved by a *more general* mechanism, and the promotion-specific composite
disappears with the compound.

**It also fixes the stale-drop hole I filed this round.** Today the promotion's
inverse restores the *verbatim typed* `TODO milk`, which never equals the fused
`TODOmilk` the previous keystroke wrote, so every earlier entry is stale-dropped.
Under (d) every entry's fingerprint is a converged value written by the same
mechanism, so the chain walks back cleanly. The escape path the ruling assumes
starts working for the first time.

---

## 2. SURVIVAL of the current 14-file diff

**Survives unchanged (the bulk, and the load-bearing part):**
- `OperationEngine` eager convergence — pre-rewrite + post-scan + composite undo.
  Under (d) it runs **unscoped**, as the coordinator says, and its post-scan arm
  becomes *more* valuable (it is what makes split/merge converge).
- `converge_keyword_headed`, and the **end-of-string arm of `keyword_headed`** —
  (d) *requires* it: `(TODO, "")` must project to `TODO` and parse back.
- `could_converge` — survives as the store's pre-filter. Under (d) it is hit less
  often, because the commit sets `task_state` explicitly and short-circuits.
- Reconciler deletion, `reingest_task_promotion_idempotent` fixed point, the F2
  cold-boot leg on `cycle_task_state_cold_boot_reingest.rs`, BugFunnel rows.
  All of these are store/ingest-side and orthogonal to the editor.

**Reworked:** `live_promotion_windowed.rs`; `type_chars.rs` (simplifies);
`promote_task_keyword_compound.rs` (majority migrates or goes).

**Deleted:** the editor-gate revert I made this round (`candidate_keyword_headed`
narrowing + the delta guard restored in `editor_view_model.rs`) becomes **moot** —
the whole proposal path goes. So the last two hours of round-2 churn is throwaway
under (d), which is itself an argument for going straight there.

**Net:** ~9 of 14 files survive as-is. The store half of #78 is not at risk.

---

## 3. RISKS

**Caret mapping (the top risk, and the only one I would budget slack for).**
`converge_input` captures `prior_cursor` and clamps it to the new text; SqlOnly
has no anchor, the Loro path anchors on the cell. On focus the buffer grows by
`keyword.len() + 1` at the FRONT, so every offset shifts. Edge cases:
- mid-word click on the display text → must land at `offset + prefix_len`;
- click *inside* the pill vs the text (already `text_center`-targeted, so this is
  mostly handled);
- **selection**: anchor and head both shift; a selection spanning the whole
  display text should not silently swallow the keyword;
- **IME**: `replay_pending_directive` defers convergence during composition, so a
  seed cannot land mid-composition — that guard already exists and covers it;
- the Loro anchor is computed on cell text (content only) while the buffer is
  raw — the anchor→buffer offset conversion is the fiddly bit.

**Fixed-point failures — three real ones, none fatal, all testable:**
1. **Out-of-vocabulary `task_state`** (the F3 class): a block carrying `TODO` in
   a `#+TODO: NEXT | DONE` document projects to `TODO x`, which that document's
   parser reads as plain text → **focus-then-blur silently DEMOTES it**. #79's
   cycle fix stops new ones; imported/legacy rows can still hold them. Needs an
   explicit guard (project only vocabulary-valid keywords, else refuse and
   disclose) — this is the one place I would add defensive code.
2. **Leading whitespace in content**: `(TODO, " milk")` projects to `TODO  milk`,
   which parses back as `milk`. Not a fixed point. Cheap fix: canonicalize
   leading whitespace at the store, or assert it away.
3. **Trailing whitespace**: buffer `TODO milk ` commits, `canonicalize_stored_content`
   trims. Already handled by the `AdoptBaseline` echo arm (there is a dedicated
   `editor_trailing_space_echo.rs` windowed test) — but it must be re-verified
   against the raw seam, not assumed.

No escape-syntax risk: option (i)/ZWSP was rejected, so there are no escaped
forms in the tree.

**Latency (#81) — a WIN, not a risk.** (d) deletes the slowest interaction:
`promote_task_keyword` is an engine compound doing a guard read + a document
vocabulary walk + two dispatches (dogfood measured e2e p50 249ms / p95 283ms,
~2× `set_field`). It is replaced by a plain `set_field`. The "parse per commit"
is not new work — it is the `keyword_headed` check the store *already* runs in
`keyword_convergence`, and under (d) the commit carries `task_state` explicitly,
which short-circuits even the row read.

---

## 4. VERDICT

**Go straight to (d)-narrow as the #78 fix. Do not build (b′) first.**

The decisive argument: **(b′) and (d) modify the same seam, and (b′) is the
larger of the two.** (b′) = teach the echo/reseed path that a converged write is
a canonicalization — i.e. do (d)'s raw-seeding work — *and then keep* the
proposal/refusal/optimistic-strip machinery, which is the thing that has now
produced three separate defect classes (#64 text loss, this round's over-proposal
corruption, the stale-drop chain). (d) does the same seeding work and deletes
~600 lines of that machinery. Paying for (b′) buys an interim whose only extra
artifact is code (d) removes.

Arm (a) is the ship-now fallback **only if the wave cannot absorb 2–4 days** —
but note it is *anti-synergistic* with (d): (a) removes the end-of-string arm
that (d) requires, so it would have to be reverted.

### Increment order, each with its red-first rung

| Inc | Content | Red-first rung |
|---|---|---|
| **0** | `source_projection` + its inverse-of-parse proptest | `source_projection_round_trips_through_the_parser` (proptest over `(task_state, content)` under drawn vocabularies) — red because the function does not exist; mirrors `inline_marks_proptest.rs`. |
| **1** | Headless seam: `converge_editor` seeds raw; `apply_local_edit` commits raw; delete the VM proposal path | Keystone hand-authored fixture `d-narrow-type-todo-milk` (the exact `task64-promotion-loro-arm` shape that is RED today) — red at Inc 0, green here. This is the conviction trace dissolving. |
| **2** | Caret mapping on focus seed (+ selection, + the Loro anchor conversion) | `frontends/gpui/tests/editor_task_keyword_promotion.rs` rewritten: `focus_seeds_the_keyword_and_maps_the_caret` with mid-word and selection arms — red before the mapping. |
| **3** | Out-of-vocabulary `task_state` guard (risk 1) | `a_task_state_the_document_does_not_declare_is_not_projected` — red-for-the-right-reason by seeding the F3 row. |
| **4** | Windowed rung rewrite + deletion sweep of the compound and its tests | `live_promotion_windowed.rs` rewritten; the deletion is proven by the suites staying green. |

Inc 0–1 are the majority of the value and are ~1 day; Inc 2 is where the slack
belongs; Inc 3–4 are mechanical.

**Recommended immediate action for #78:** hold this diff unwoven (its store half
is correct and (d) keeps it), and open (d) as the follow-on lane. If Martin wants
the F2 store fix landed independently, the clean cut is to land everything in
this diff *except* the editor-facing pieces, with the one RED fixture carried as
a named known-red pointing at (d) — not silenced.
