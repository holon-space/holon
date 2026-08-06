# Keystone known reds — the full-depth registry

The per-weave land gate is `just keystone-smoke` (ONE proptest case). The
full-depth sweep (`just pbt general`, default 64 cases) reaches sequences the
smoke never draws, and today it fails roughly half its runs from the
pre-existing signatures below. That is why full depth runs as a NIGHTLY tier
(`just keystone-nightly`) instead of per-weave, and why the nightly is judged
against this registry rather than against "zero failures".

**The discipline:**

- A failure whose signature matches a row below is a **pass-with-note**: the
  nightly prints it as a `WARN` line and still exits 0.
- **ANY other signature is a regression to triage** — the nightly exits
  non-zero and prints the novel signature verbatim. Do not add a row to silence
  it; triage it first (`bug-gap-triage`), and only register it here if Martin
  ratifies it as a known red.
- A row is **removed** when its fix lands with a green soak (repeated
  full-depth runs no longer producing that signature). Rows are not archived
  in place — the registry describes what is red NOW.
- Between the fix landing and the soak confirming it, a row sits at status
  `fixed-pending-soak`. Only `known-red` rows classify, so from that moment a
  recurrence of the signature is reported as NOVEL — the fix is treated as
  believed, not proven, and the tier stops absorbing that signature silently.
  Delete the row once the soak is green; revert it to `known-red` (with the new
  payload decoded) if the soak brings it back.
- **The target end state is a MINIMAL registry of OWNED residuals, not
  necessarily an empty one** (Martin, 2026-08-04). The known-reds fix program
  closes when every family is either FIXED or is a low-frequency residual
  (order of one lifetime observation) carrying all three of: a corrected
  evidence trail decoded from a real payload, a NAMED owner, and a resurfacing
  trigger — a registered `Match pattern` that flips it back to open the moment
  it recurs. A residual without an owner and a trigger is not a residual, it is
  an unclosed family. Absence from a soak never closes a family with fewer than
  ~5 lifetime observations; those need a deterministic repro instead.

**How the matching works:** `scripts/keystone-known-reds.sh` parses THIS FILE.
The `Match pattern` column is the single source of truth for classification —
an extended-regex (`grep -E`) applied to each extracted failure signature line.
Editing a pattern here changes the nightly's verdict; there is no second copy
in the script or the justfile. A pattern may not contain `|` — that is the
markdown table separator; use character classes instead of alternation.

## End-state audit — known-reds fix program, 2026-08-05

Status of Martin's queue item *"fix ALL known-red families"*: **NOT yet
discharged.** Of the six families this program touched: three are FIXED pending
soak, one is half-fixed (`org-blocks-ref-diverge` cause A fixed, cause B
`::img::0` OPEN — found by this program's own sweeps), and two remain open
(`sidebar-focus-bind`, `split-id-no-pairing`/`syn-real-mint`). Seven families
remain open in the registry overall, including the four untouched
Loro/caret/pinblock rows and the UNOWNED `page-without-own-file` singleton.

**FIXED, pending soak** (fix landed with a red-first proof; row deletes once a
full-depth soak is green — soak absence alone is not the bar for any of them):

| Family | Cause | Lock |
|---|---|---|
| `org-render-echo-loop` | renderer emitted an unconditional blank line after a body | unit `render_of_parsed_disk_text_is_byte_identical` |
| `watch-rows-cdc-parent` | oracle parented template DEFINITION blocks at `no_parent` to win seed exclusion; the watch invariant applies no seed filter | keystone `watch-rows-template-child-parent` |
| `state-toggle-row-absent` | oracle decided panel visibility with a bare ancestor walk, ignoring the page stop and depth cap | unit `main_panel_visibility_stops_at_a_non_root_page` |

**Half-fixed — row stays open because a SECOND cause is live:**

- `org-blocks-ref-diverge` — cause A (undo over-reverts file-ingested content
  blocks) FIXED, locked by keystone
  `undo-over-reverts-file-ingested-content-blocks`. Cause B (`::img::0` sub-block
  id remap; non-empty `only_in_ref`) OPEN, attribution pending a base-rev A/B.

**OPEN** (owner and resurfacing trigger per item; one is still UNOWNED) (the residual policy above):

- `sidebar-focus-bind` — unfixed. Its `Match pattern` was CORRECTED this program
  (risk R10 materialised: an intervening landing rewrote the assertion, so the
  classifier was reporting a registered known red as a NOVEL regression and the
  nightly would have failed the tier for it).
- `split-id-no-pairing` / `syn-real-mint` — unfixed and NOT REPRODUCED at this
  base. The plan's lead cause (an N-bounded per-tick reconcile, "the 4th of 4
  splits") is REFUTED: decoding the full shrink shows the unpaired id stays
  `split-3` as earlier splits are removed, and the fully minimized panic is
  `split-0` with `Mapped: []` — one split, nothing paired. `block::split-N` is
  minted off `block_state.next_id`, which several transitions bump, so N does not
  index splits. Green control `four-splits-one-tick-all-pair` locks the shape on
  both the default and the corpus wiring.
- `page-without-own-file` — 1 lifetime observation, **UNOWNED**: it does not yet
  satisfy the residual policy and needs a triage owner.
- `loro-frontier-height`, `loro-stable-id-missing`, `editor-caret-mirror`,
  `pinblock-unrendered-target` — untouched by this program; owners in the table.

**Standing caveat:** `editor-caret-mirror` cannot be judged by any soak until
mirror-bearing wirings can be pinned — its engagement is `0/N` in Loro-only
draws, so a green run is evidence of DESELECTION, not of a fix.

## Registry

| Key | Status | Match pattern | Signature | Evidence | Task | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| `syn-real-mint` | known-red | `per-tick reconcile: one synthetic per minted real id` | `harness.rs` assertion `per-tick reconcile: one synthetic per minted real id (syn=[], real=[...])` — the per-tick synthetic→real id reconcile finds a real block minted with no synthetic counterpart. | Pre-existing since ≤2026-07-25; also fires in the windowed (GPUI) keystone, so it is not headless-only. | #62 | keystone id-reconcile (task #62) |
| `org-render-echo-loop` | fixed-pending-soak | `diverged from the oracle: .*"inv-org-render-fixed-point"` | `[inv-org-render-fixed-point] render != disk PERSISTED for <budget> — … a real echo-loop / oscillation`. The decoded instance is a ONE-BYTE difference: disk 277 bytes vs rendered-from-SQL 278, identical except a blank line the renderer emits between a multi-line block body and the next heading, which the parser does not write back. | Decoded 2026-08-04 from `fixture-logs-2026-07-31/keystone-nightly-20260731-083505-run2.log.zst` (85 panics). SUPERSEDES the prior "reproduces around empty-title headings" attribution — that was never true of any captured payload. All 85 corpus panics are the same shape: render−disk = exactly +1 byte, all on `forward-edge-page.org`. **FIXED 2026-08-04**: `render_headline_block` (`crates/holon-org-format/src/models.rs`) pushed a second newline after every non-empty body; it is now emitted only when the body's last line starts a list item (`body_needs_list_terminator`). The unconditional REMOVAL was tried first and refuted by verification — that blank line is load-bearing org syntax after a list-like line, and without it a following `#+BEGIN_SRC` child is swallowed into the list and LOST on write-back → ingest (real data loss on the `FileFormatAdapter` seam). Red-first proof: `holon-orgmode` `render_of_parsed_disk_text_is_byte_identical` (the 277-byte corpus fixture) RED at 278 bytes → GREEN; `blank_line_closing_a_list_body_survives_round_trip` locks the other half. Closed gate gap: `org_block_round_trip_pbt` — the only binary driving the adapter seam — was in NO `just` recipe, which is why the refuted attempt passed every gate; `just pbt orgmode` now runs it. Status stays `fixed-pending-soak` until a full-depth soak confirms the signature is gone; the classifier already treats a recurrence as NOVEL. The regression lock is that unit test, NOT a keystone JSONL case: write-back and the `render_org` tool share one `WritebackRenderer`, so an ordinary edit converges disk and render by construction — three probe shapes (multi-line body + following sibling, on the seeded forward-edge page and on journals) all replayed GREEN with the defect deliberately re-added. Reproducing it composed needs a second, unrelated ingredient (a write-back that does not land — veto/quarantine or an incremental splice), so a keystone case would lock that conjunction and go red for another family's reason once this one is fixed; disclosed deviation, ruled 2026-08-04. | #66 | org round-trip / holon-org-format (task #66) |
| `org-blocks-ref-diverge` | known-red | `diverged from the oracle: .*"inv-blocks-match-ref/[a-z_]+".*fields diverge from reference` | `[inv-blocks-match-ref/org]` reports `fields diverge from reference`. Decoded instance: SUT-org 33 blocks vs reference 31, `only_in_ref` EMPTY, the two extras (`block:7-cn4c9-9js-vi026hjesuj2-3`, `block:b-5-fx-g-k3x54-7v-8x`) both parented to `block:ref-doc-2` — a set-membership excess on the SUT side confined to one externally-ingested ref-doc, NOT a field diff. | Decoded 2026-08-04 from `fixture-logs-2026-07-31/keystone-nightly-20260731-191108-run1.log.zst` (123 panics — the dominant family). SUPERSEDES the prior JOURNALS-PROJECTION attribution: no journals block appears in the divergence. **VERDICT 2026-08-04: ORACLE bug — the reference's undo over-reverts file-ingested CONTENT blocks.** Two facts the first decode missed, both re-derived from the archived payload: (a) the SAME two ids are simultaneously spurious in `inv-blocks-match-ref/block_raw` AND `/matview`, so this is neither an org-projection defect nor the `!b.is_page()` ref-vs-SUT filter asymmetry the plan suspected — the reference simply does not hold the blocks, on every store; (b) proptest shrank the counterexample to THREE transitions, `CreateBlockUnderFocus → WriteOrgFile → UndoLastMutation`. Mechanism: `CreateBlockUnderFocus` is User-origin and pushes oracle undo snapshot S0 taken BEFORE the file exists; `WriteOrgFile`'s watcher re-ingest is INGEST-origin in prod (`operation_engine.rs:1220` — only User-origin ops push undo entries) so `engine.undo()` leaves its blocks alone, while the oracle's `seed_org_file` writes them INTO the snapshotted `block_state`; `pop_undo_to_redo` restores S0 and drops them, hence `only_in_ref` EMPTY and zero field deltas. This is the CONTENT-BLOCK sibling of the already-fixed doc-root case: `rematerialize_file_ingested_docs` fixed exactly this class for the file's ROOT page and explicitly left "any USER-origin children remain undone" — but a file's parsed content blocks are INGEST-origin, not user-origin, so that half stayed open. **FIXED 2026-08-04**: `files.ingest_origin_blocks` (a `BTreeSet` in `FileAdapterState`, outside the undo snapshot like `documents`/`next_doc_id`) records every block an ingest materialises (`insert_document`, `seed_org_file`, `bulk_add_blocks`; cleared on file rewrite and on `remove_document`), and `rematerialize_file_ingested` restores each such block VERBATIM from the pre-undo state when the snapshot restore dropped it. Restoring from the pre-undo state rather than from the ingest is what makes it prod-faithful: a block the user genuinely deleted is absent there and stays gone, and one a later user op edited was re-snapshotted by that op so it never reaches the path. NOT a weakening — no tolerance, wildcard or skip was added; if prod wrongly drops a file-ingested block on undo the invariant now reports it as `only_in_ref`. Red-first proof: hand-authored `undo-over-reverts-file-ingested-content-blocks` RED with `only in inv-blocks-match-ref/org (2): [block:ext-a, block:ext-b]` / `only in reference (0): []` / `field deltas (0)` on org + loro + block_raw + matview → GREEN; `ref-doc-0-undo-over-reverts-file-ingested-doc` (the doc-root half) stays green. Gate: `just hand-authored` 32/32 x3 consecutive; `just keystone-smoke` x3 substantive (`inv-blocks-match-ref/org` engagement 20/20, 31/31, 18/18). **THE FAMILY HAS TWO CAUSES; only cause A is fixed, so the row stays `known-red`.** Cause A (undo over-reverts file-ingested content blocks) is fixed and locked by the hand-authored case, so a regression of A is caught by `just hand-authored`, not by this row. **Cause B, OPEN, first seen 2026-08-05** in the second 64-case sweep: `inv-blocks-match-ref/org` with EQUAL counts on both sides (18 vs 18, 12 vs 12, 19, 21) and ONE id on each side — `only_in_actual` `block:<uuid>::img::0`, `only_in_ref` `block:<slug>::img::0`. That is the same logical image sub-block under two ids: the PARENT was remapped synthetic→real but the derived `::img::0` child id was not. It is the id-space-mangling cause the plan's F2 dossier refuted for cause A's payload (`only_in_ref` was empty there) and explicitly told us to keep in mind for other instances — non-empty `only_in_ref` is its signature. ATTRIBUTION OPEN: `::img::0` appears in NONE of the archived 2026-07-31 corpus logs, and the FIRST 64-case sweep of 2026-08-05 (same code) did not show it either, so it is intermittent and is either newly unmasked by cause A's fix (runs now get further) or newly landed by an intervening change. Needs a base-rev A/B before it is attributed — do NOT assume it is a regression of this lane's work, and do not assume it is not. Because the `Match pattern` is shape-blind (it cannot use alternation), this one row necessarily covers both causes. | #76 | keystone oracle — ref undo model (task #76) |
| `split-id-no-pairing` | known-red | `resolve_sut_id: oracle-only id .* has no SUT pairing` | `types.rs` `resolve_sut_id: oracle-only id block::split-N has no SUT pairing … Mapped: [block::split-0, block::split-1, block::split-2]` — the first three editor splits reconciled, the fourth did not. The mirror signature of `syn-real-mint` (that row is a real id with no synthetic; this is a synthetic with no real). | 52 panics in `fixture-logs-2026-07-31/keystone-nightly-20260731-193535-run1.log.zst`. Registered 2026-08-04 — it had been firing unregistered, so the nightly could not issue a verdict. | #62 | keystone id-reconcile (task #62) |
| `watch-rows-cdc-parent` | fixed-pending-soak | `diverged from the oracle: .*"inv-watch-rows-match-ref".*CDC parent_id mismatch` | `CDC parent_id mismatch for block:tpl-c1 in watch '<id>': actual_ui_model=Some("block:tpl") expected=Some("__document_root__")` — a template child whose CDC-delivered `ui_model` parent is the template block while the reference still holds the document-root sentinel. | 23 panics in `fixture-logs-2026-07-31/keystone-nightly-20260731-193535-run1.log.zst`. Registered 2026-08-04. **VERDICT 2026-08-05: ORACLE bug — CONFIRMED, mechanism exact.** Reproduced live at this base (a 16-case full-depth run) and minimized by proptest to just `[InstantiateTemplate, SetupWatch]`. `block:tpl-c1` is not an instantiated child at all — it is the template DEFINITION child that `InstantiateTemplate`'s driver seeds UNDER `block:tpl`. The oracle parented BOTH definition blocks at `no_parent`, a deliberate documented shortcut whose only purpose was to make `seed_block_ids` classify them as SEED and drop them from the block-comparison invariants. But `inv-watch-rows-match-ref` applies NO seed filter — an `AllBlocks` watch returns every block — so the shortcut leaked out as `expected=__document_root__` (the `normalize_parent` image of `no_parent`) against prod's structurally-correct `block:tpl`. Prod was right throughout. **FIXED 2026-08-05 per Martin's ruling (resolve REAL parents, do NOT wildcard the sentinel):** new cap `seed_template_definition` seeds the definition child under the definition root so `parent_id` is truthful everywhere it is observed, and forces ONLY that block's `block_documents` entry to `no_parent` so the seed classification is unchanged. Parent truthfulness and seed classification are independent axes; the old code conflated them. The rejected alternative — treating `__document_root__` as a wildcard in `normalize_parent` — would also have hidden a genuinely wrong parent, which is exactly what this invariant exists to catch. Red-first proof: hand-authored `watch-rows-template-child-parent` RED with the registry signature verbatim (`CDC parent_id mismatch for block:tpl-c1 in watch 'query-tplparent': actual_ui_model=Some("block:tpl") expected=Some("__document_root__")`) under a mutation probe restoring the old `no_parent` seeding → GREEN with the fix. Gate: `just hand-authored` 33/33 x3 consecutive; the depth-16 run that produced the repro is green after, `inv-watch-rows-match-ref` engagement 9/9 and 40/40 (the invariant is trivially `Ok` with no watch registered, so the fixture registers a `parent_id`-carrying watch to exercise the parent arm). Status stays `fixed-pending-soak` until a full-depth soak confirms it. | #76 | keystone oracle — ref watch model (task #76) |
| `state-toggle-row-absent` | fixed-pending-soak | `could not resolve the state_toggle cycle intent` | `components.rs` `[toggle_state] click #1 failed for <block>: … in region main within 2s. <block> renders NO node in region main — the panel is not showing this block.` The reference believes a block is present and toggleable in the main panel; the panel does not render it. Same defect surface as `inv-main-panel-rows-match-focus`'s dropped-row arm, hit by the driver before the invariant can report it. | 23 panics in `fixture-logs-2026-07-31/keystone-nightly-20260731-191108-run2.log.zst`. Registered 2026-08-04. **VERDICT 2026-08-05: ORACLE bug — and the TIMING ARM DOES NOT APPLY, so nothing files to the latency track.** Martin's ruling conditioned the latency filing on "if the missing row appears at 10s". It never appears, at any deadline: the corpus payload shrank to `[SplitBlock, SplitBlock, Indent, BlockToPage, ToggleState]`, and `BlockToPage` mints a NEW page and re-homes the origin's children under it. The compiled main-panel query STOPS descending at any non-root page, so prod is RIGHT to render no row — waiting longer cannot change that. Only 8 entities were rendered, which also rules out the depth-20 cap of ledger entry 14 (that needs 20+ nesting). The defect is that the oracle decided main-panel visibility with a bare parent-chain walk (`is_descendant_of_any`) that honours neither the page stop nor the depth cap, so it offered a click target no user could click — the driver then burned its full 2s poll and dispatched nothing. **FIXED 2026-08-05** by one predicate, `RefBlockTree::main_panel_renders`, implemented via the panel query's own traversal (`descendant_within_stopping_at_pages` + the new single-sourced `query::MAIN_PANEL_MAX_DEPTH`), used by BOTH `main_editable_descendants` (the generator candidate set AND `inv-main-panel-rows-match-focus`'s required set) and `ToggleState`'s visibility precondition, which previously duplicated the wrong walk and said so in its own comment. This closes BOTH arms of the F5 cluster the plan predicted were one family. It also required fixing a real off-by-one in `descendant_within_stopping_at_pages` itself: it tested whether the CURRENT node is a page, but the CTE guard is on the node being descended FROM (the parent) — so the page stop never fired when the page was a DIRECT CHILD of the root, which is exactly the shape `BlockToPage` creates. DISCLOSED: this narrows an invariant's required set, which is a weakening in form; it is a correction in substance because the panel query's traversal IS the specification of what the panel renders, and demanding rows outside it asserted something false. The depth half was already the sanctioned action in ledger entry 14 ("un-comment once the candidate set honours the same depth cap"); this lane did NOT touch any pin, ceiling, or the quarantined entry-14 fixture — that stays with the read-budget lane. Regression lock is the unit test `main_panel_visibility_stops_at_a_non_root_page`, NOT a keystone JSONL case: the fix is a PRECONDITION/candidate-set narrowing and the hand-authored runner replays its sequence verbatim without evaluating preconditions, so a composed case naming the unrenderable target reproduces the signature (verified — it did, verbatim) but can never go green. Disclosed deviation, same shape as `org-render-echo-loop`'s. | #77 | main-panel projection (task #77) |
| `page-without-own-file` | known-red | `diverged from the oracle: .*"inv-every-page-has-its-own-file"` | `[inv-every-page-has-its-own-file] 1 page(s) not homed to exactly one own file: ["page <uuid> owns NO file (fileless — content lives only in th…"]` | 1 panic across the 2026-07-31 corpus. Registered 2026-08-04 for verdict completeness; a single lifetime observation, so soak absence alone must not close it. | — | UNOWNED — needs a triage owner before it can count as a residual |
| `loro-stable-id-missing` | known-red | `missing STABLE_ID metadata` | `Node TreeID { peer: …, counter: … } missing STABLE_ID metadata` — a Loro tree node reached without the stable-id metadata the backend requires. Suspected same shallow-snapshot/history-trimming class as `loro-frontier-height`. | 1 panic across the 2026-07-31 corpus. Registered 2026-08-04. | #78 | Loro backend (task #78) |
| `editor-caret-mirror` | known-red | `diverged from the oracle: .*"inv-editor-caret/mirror".*Caret mismatch` | `[inv-editor-caret/mirror] Caret mismatch on <block>: reference model cursor_byte=…, SUT tracked caret=…`. Only reachable in wirings that mirror the editor — engagement is 0 in Loro-only draws, so absence in a green run is not evidence of a fix. | Task #66 family, NEW 10. | #66 | editor mirror wiring (task #66) |
| `sidebar-focus-bind` | known-red | `LeftSidebar never bound a navigation.focus` | `components.rs` `[await_sidebar_intent] LeftSidebar never bound a navigation.focus (the row's ``action:``) click-intent for <block> with modifiers ClickModifiers { … } within 5s` — the sidebar's nested live_block watch fails to stream the target's selectable; latent arrival-order sensitivity, amplified (not created) by the reverted ORDER-BY snapshot change. | Fired 3× in a 64-case run at main-based tree 2026-07-31, and again 2026-08-05 (64-case run, `block:structural-page`, primary click). **PATTERN CORRECTED 2026-08-05 — risk R10 materialised.** An intervening landing rewrote the assertion to interpolate which wiring was awaited (`navigation.focus (the row's ``action:``)` vs `navigation.open_tab (…)`), and moved it from `SutFocusWrite::apply_navigate_focus` to `await_sidebar_intent` (`components.rs:1337`). The old pattern quoted `navigation.focus click-intent` as ADJACENT words, so it stopped matching and the classifier reported this known red as a NOVEL regression — i.e. the nightly would have failed the tier for a family that is registered. The pattern now stops at `navigation.focus`; it deliberately does NOT cover the `navigation.open_tab` variant, which is a different failure mode and correctly classifies as novel until triaged. Still OPEN as a family (unfixed) — this is a registry-truth correction, not a closure. | #77 | sidebar click-intent binding (task #77) |
| `pinblock-unrendered-target` | known-red | `PinBlock.sql_reads: [0-9]+ exceeds expected 17` | `[inv-sql-budget PINNED] PinBlock.sql_reads: N exceeds expected 17 + tolerance 1 = 18`, N ≈ 90–141, wall ≈ 2.2s, spans ≈ 520–820. Co-fires with `inv-focus-roots` (right_sidebar) and `inv-main-panel-rows-match-focus`. | Ledger entry 14, RESOLVED 2026-08-04 by the measurement lane (+ verifier round): NOT a symptom of the focus-roots red and NOT a cost-model gap — all three reds share ONE cause, a pin target whose NESTING DEPTH exceeds the main-panel query's depth-20 recursion cap (`WHERE _vl2.depth < 20 … AND _vl2.depth <= 20`, documented at `crates/holon/tests/turso_storage_repros/tabs_main_panel_delivery.rs:130`). Past the cap the panel renders no row for it, so `click_entity_with_modifiers` (`user_driver.rs:719`) spins its 2s poll — 41 redundant re-snapshots of two `watch_view` SELECTs — and the pin never dispatches. Depth 12 → 17 reads; 21 → 89; 22 → 101. Width is irrelevant: a 40-block FLAT panel renders all 40 rows and pins the 40th for 17 reads. Oracle side, `main_editable_descendants()` applies no depth filter, so the generator offers targets the panel query truncates. Do NOT widen the ceiling. | #7 | read-budget measurement lane (task #7) |
| `loro-frontier-height` | known-red | `frontier change present` | Panic at `loro_backend.rs` `doc_lamport_height`: `doc.get_change(id).expect("frontier change present")` — a frontier id whose change is not retrievable (suspect: shallow-snapshot history trimming). | Fired 1× in a 64-case run at main-based tree 2026-07-31. | #78 | Loro backend (task #78) |

## `cargo test -p holon-integration-tests --lib --features pbt` known reds

Separate suite from the composed nightly above (this is the crate's own unit
tests, not `general_e2e_composed_pbt`), tracked here for the same
pass-with-note discipline. `scripts/keystone-known-reds.sh` only classifies
composed-nightly logs, so it does not consume these rows automatically — they
are a manually-maintained baseline for judging this suite's local runs.

**Baseline established 2026-08-06** from three consecutive runs at
main=6ec42f1a (`/opt/homebrew/opt/parallel/bin/parallel --semaphore --id
holon-build -j4 --fg -- cargo test -p holon-integration-tests --lib --features
pbt`):

| Run | Wall time | Passed | Failed |
| --- | --- | --- | --- |
| 1 | 89.09s | 291 | 10 |
| 2 | 192.23s | 270 | 31 |
| 3 | 67.39s | 290 | 11 |

Run 2's wall time is >2x run 1/3's — a symptom of concurrent machine
contention (other lanes sharing the `holon-build` semaphore slot), not a
change in the suite. Its 21 extra failure names beyond run 1's set are NOT
registered below: they never recurred in run 3 (a clean, non-contended run),
so a single contended sample is insufficient evidence they are real
intermittent reds rather than contention artifacts (timeouts, resource
starvation) — see the flagged list at the end of this section. Both run 1 and
run 3 are close in wall time to each other and neither shows machine
contention symptoms, so their intersection is treated as the stable baseline.

**Stable family — right-sidebar pin/focus-roots (9 tests, identical across
all 3 runs)**, attributed to the SAME region-literal typo documented at
`docs/Testing/BugFunnel.md:575` (OPEN/ESCALATED, uncorrected): the seed GQL
`default-right-sidebar::src::0` filters `fr.region = 'right'` while
production `focus_pin` writes `navigation_history.region = 'right_sidebar'`
(`Region::RightSidebar.as_str()`), so the literal never matches and the right
sidebar never shows a pin, in prod and in every SUT/oracle path that exercises
it:

| Key | Status | Match pattern | Signature | Evidence | Task | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| `lib-nav-history-shift-action-region` | known-red | `the block bullet's shift_action pins into the right sidebar only` | `components.rs:2756` `pbt::frontend_slice::components::tests::headless_nav_history_ops_dispatch`: `assertion `left == right` failed: the block bullet's shift_action pins into the right sidebar only` / `left: Main` / `right: RightSidebar`. | Present verbatim in run1 (`taska_lib_run1.log:379-382`), run2, and run3 (`taska_lib_run3.log`) of the 2026-08-06 baseline. | BugFunnel.md:575 | region-literal fix (unowned, escalated) |
| `lib-pin-block-right-sidebar-probe` | known-red | `focus_pin\(right_sidebar, .*\) must populate focus_roots\(right_sidebar\)` | `components.rs:5268` `pbt::frontend_slice::components::tests::headless_pin_block_right_sidebar_probe`: `[pin-probe] headless focus_pin(right_sidebar, block:ref-block-0) must populate focus_roots(right_sidebar) — the matview did not update without a window; got []`. | Present in all 3 runs of the 2026-08-06 baseline (e.g. `taska_lib_run1.log:412-415`). | BugFunnel.md:575 | region-literal fix (unowned, escalated) |
| `lib-unpin-block-probe` | known-red | `no right-sidebar pin row for ref-block-0 in navigation_history` | `components.rs:5397` `pbt::frontend_slice::components::tests::headless_unpin_block_probe`: `[unpin-probe] no right-sidebar pin row for ref-block-0 in navigation_history`. | Present in all 3 runs of the 2026-08-06 baseline (e.g. `taska_lib_run1.log:421-424`). | BugFunnel.md:575 | region-literal fix (unowned, escalated) |
| `lib-focus-roots-mismatch-right-sidebar` | known-red | `region 'right_sidebar' focus_roots mismatch .* the matview faithfully reflects the BASE navigation_history table` | `navigation_pbt.rs:650` `pin_block_lockstep_stays_green` and `navigation_pbt.rs` (`harness.rs:855`) `frontend_navigation_pbt`: `lockstep PinBlock should be green: [("inv-focus-roots", "[inv-focus-roots] region 'right_sidebar' focus_roots mismatch — the matview faithfully reflects the BASE navigation_history table, which itself disagrees with the reference. …")]` with `expected: {"block:ref-block-0"}` / `mirror/matview/base: {}`. | Present in all 3 runs of the 2026-08-06 baseline (e.g. `taska_lib_run1.log:432-435`). Covers 2 test names — both emit this exact `inv-focus-roots` payload. | BugFunnel.md:575 | region-literal fix (unowned, escalated) |
| `lib-sut-only-pin-not-caught` | known-red | `SUT-only PinBlock must trip inv-focus-roots with a Fail; failures were \[\]` | `navigation_pbt.rs:685` `pbt::frontend_slice::navigation_pbt::teeth::sut_only_pin_block_is_caught_by_focus_roots`: `SUT-only PinBlock must trip inv-focus-roots with a Fail; failures were [], ran=[...]` — the invariant that should catch a SUT-only pin never fires (because the base row is never written into the region the invariant checks). | Present in all 3 runs of the 2026-08-06 baseline. | BugFunnel.md:575 | region-literal fix (unowned, escalated) |
| `lib-right-sidebar-renders-pins` | known-red | `both pinned blocks must render in the right sidebar \(apple=None, zebra=None\)` | `structural_pbt.rs:4321` and `structural_pbt.rs:4341` (`right_sidebar_renders_pins`, `right_sidebar_renders_pins_in_declared_added_ts_order`): `both pinned blocks must render in the right sidebar (apple=None, zebra=None); rendered right-sidebar entity order = ["block:default-right-sidebar"]` — the first also names the cause verbatim in-message (`the seed region filter drifted off Region::RightSidebar.as_str() ('right_sidebar')`). | Present in all 3 runs of the 2026-08-06 baseline. Covers 2 test names sharing this message prefix. | BugFunnel.md:575 | region-literal fix (unowned, escalated) |
| `lib-sidebar-pages-declared-order` | known-red | `both seeded sidebar pages must render \(apple=None, zebra=None\)` | `structural_pbt.rs:4175` `pbt::frontend_slice::structural_pbt::teeth::sidebar_renders_pages_in_declared_content_order`: `both seeded sidebar pages must render (apple=None, zebra=None); rendered sidebar entity order = ["block:default-left-sidebar"]`. Co-occurs with the right-sidebar family in all 3 runs and shares its `apple`/`zebra` pin-fixture naming, but the causal link to the `'right'`/`'right_sidebar'` literal typo has NOT been independently traced here (only correlated) — flag for attribution review before assuming it is the same root cause. | Present in all 3 runs of the 2026-08-06 baseline. | BugFunnel.md:575 (correlated, unverified) | region-literal fix (unowned, escalated) |

**Separate stable red — unrelated to the above family:**

| Key | Status | Match pattern | Signature | Evidence | Task | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| `lib-seed-wide-drift` | known-red | `scripts/seed_wide/index.org body drifted from assets/default/index.org` | `live_mcp.rs:1223` `pbt::composed::live_mcp::tests::seed_wide_stays_aligned`: `assertion `left == right` failed: scripts/seed_wide/index.org body drifted from assets/default/index.org (the DEFAULT_INDEX_ORG the iOS app boots)`. Decoded diff: `assets/default/index.org` carries `:WIDGET_ONLY: t` on `default-left-sidebar`, `default-main-panel`, and `default-right-sidebar`; `scripts/seed_wide/index.org` does not — the two files have diverged. | Present in all 3 runs of the 2026-08-06 baseline. | — | UNOWNED — needs a triage owner |

**Intermittent — reproduced in 2 of 3 runs (present in a CLEAN run, so not
purely a contention artifact; absent from run 1):**

| Key | Status | Match pattern | Signature | Evidence | Task | Owner |
| --- | --- | --- | --- | --- | --- | --- |
| `lib-displayed-text-nested-content-skipped` | known-red | `must reach Ok over the grafted nested content .* got Some\(Skipped\("\[inv-displayed-text/viewmodel\] no block-bound text widgets` | `integration_tests.rs:179` `pbt::frontend_slice::integration_tests::frontend_slice_displayed_text_viewmodel_bites_on_nested_content`: `headless /viewmodel must reach Ok over the grafted nested content (NOT Skipped — that would mean the recursive snapshot didn't resolve the Main-panel content), got Some(Skipped("[inv-displayed-text/viewmodel] no block-bound text widgets in the VM tree yet"))`. | Absent from run 1 (89s, clean); present in run 2 (192s, contended) AND run 3 (67s, clean) of the 2026-08-06 baseline with byte-identical panic text — reproduced in a non-contended run, so not registered as contention-only. Observed 2 of 3 runs; do not read this as a 67% rate, it is 2 samples. | — | UNOWNED — needs a triage owner |

**Observed once, in the contended run only — NOT registered, needs separate
triage before either fixing or adding to this file** (all 21 names below
appeared ONLY in run 2 of the 2026-08-06 baseline and did not recur in the
clean run 3; treat as likely contention-induced — e.g. a driver poll timeout
under CPU starvation — not evidenced as genuine intermittent reds from a
single contended sample):
`headless_type_chars_commits_to_block_raw`,
`booted_widget_tree_has_no_pending_placeholders`,
`editor_sut_only_type_chars_is_caught`,
`editor_type_chars_lockstep_stays_green`,
`journal_feed_expanded_newest_first_with_divider`,
`org_ingest_link_marks_survive_full_catalog`,
`shadow_mesh_predicts_concurrent_primary_peer_merge`,
`wide_create_document_lockstep_stays_green`,
`wide_frontend_setup_watch_lockstep_stays_green`,
`wide_frontend_sut_only_navigate_is_caught`,
`wide_frontend_sut_only_toggle_state_is_caught`,
`wide_frontend_sut_only_watch_rows_is_caught`,
`wide_frontend_toggle_state_lockstep_stays_green`,
`wide_indent_outdent_roundtrip_lockstep`,
`wide_indent_then_split_parent_lockstep`,
`wide_pin_block_lockstep_stays_green`,
`wide_simulate_restart_lockstep_stays_green`,
`wide_split_then_type_lockstep_stays_green`,
`wide_sut_only_create_document_is_caught`,
`wide_sut_only_pin_block_is_caught`.
None of these match the three names from an earlier session's unverified
"rotating 10th" premise (`editor_type_chars_lockstep_stays_green`,
`wide_create_document_lockstep_stays_green`,
`turso_draw_reaches_the_feed_driven_writeback_path`) as a stable rotating
set — `editor_type_chars_lockstep_stays_green` and
`wide_create_document_lockstep_stays_green` DO appear in this contended-only
list, but `turso_draw_reaches_the_feed_driven_writeback_path` never failed in
any of the 3 runs, and neither of the two that did appear recurred in the
clean run 3. That premise is not re-confirmed by this baseline.

## Evidence corpus & the pattern-drift guard

Every row's Evidence column must name a **decoded payload**, not a recollection.
Two rows were found on 2026-08-04 to describe something no captured payload ever
showed (`org-render-echo-loop` blamed empty-title headings; `org-blocks-ref-diverge`
blamed the journals projection), and both would have mis-steered a fix. So the
raw logs those verdicts came from are committed, zstd-compressed, at
`crates/holon-integration-tests/hand-authored-regressions/fixture-logs-2026-07-31/`
— four red runs and the four green runs of the same nights (the 4/8 = 50%
base-rate record). They are evidence, not scratch: `/tmp` does not survive a
reboot.

`just known-reds-fixture` replays the four red logs through
`scripts/keystone-known-reds.sh` and asserts the per-key hit counts are exactly
what the corpus contains. Since the corpus is immutable, a diff there means a
`Match pattern` above no longer matches the text it was written for — i.e.
someone reworded an assertion message and silently broke the classifier.
Re-bless with `scripts/keystone-known-reds-fixture.sh --bless` **only** after
confirming the pattern change is intended.

Run it whenever you touch a panic/assertion message that a pattern above quotes.

## Where it runs — local, not GitHub Actions

The tier is a LOCAL nightly (Martin's machine or an orchestrator session). No
scheduled workflow was added, because CI cannot currently execute the composed
keystone at all:

- `.github/workflows/ci.yml`'s `rust-checks` job runs
  `cargo test --workspace --exclude rust_lib_holon` on `ubuntu-latest`, and
  `pbt` IS a default feature of `holon-integration-tests`, so in principle the
  keystone is in that job's scope.
- In practice it never gets there. The last 200 CI runs are 200 failures; the
  step spends ~14min compiling and then dies inside the `holon` crate's own
  suite (`create_page_from_link`), before any `holon-integration-tests` binary
  starts. `general_e2e_composed_pbt` appears in ZERO CI logs.
- Full depth is hours of wall clock on top of that compile, on a 2-core runner.

A scheduled job today would be a gate that never ran the keystone. Re-evaluate
once CI is green and the runner budget is measured; until then
`just keystone-nightly` IS the tier.

## Running the tier

```
just keystone-nightly            # 2 serialized full-depth runs, judged against this file
just keystone-nightly 1 8        # 1 run at 8 cases — for exercising the plumbing, not a gate
```

Keystone runs must be serialized against every other keystone lane:

```
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-keystone -j1 --fg -- just keystone-nightly
```
