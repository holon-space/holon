# Verify: readonly-invariant lane — VERDICT

Workspace /Users/martin/Workspaces/pkm/holon/.claude/worktrees/readonly-invariant, @- = 830d794f878f (asserted).
Tree asserts passed. lane-logs/ + lane-report are UNTRACKED (jj status lists 38 tracked files, none lane-*).

## Per-claim

C1 artifacts — CONFIRMED (partial on self-repro).
  Invariant body crates/holon-integration-tests/src/pbt/invariants/bodies/read_only_home_refuses_writes.rs;
  transition .../transitions/attempt_read_only_edit.rs; fixture .../composed/wide_e2e.rs:270-345 (real
  keystone-recipe.cook ingested through the production FormatRegistry, ids block:keystone-recipe.cook::b::0/1);
  jsonl case a-write-to-a-read-only-homed-block-is-refused added (jj diff); Model.md invariant 14 at
  docs/Architecture/Model.md:173-197.
  Red log lane-logs/red5-35002.log (12:54, 1.3MB) contains verbatim:
    block `block:keystone-recipe.cook::b::0` is homed in a read-only file, yet `block_raw` no longer holds
    what the ingest wrote: ingested ... ; test result: FAILED. 8 passed; 1 failed
  Probe revert verified by sha256: crates/holon/src/api/operation_dispatcher.rs ==
  lane-logs/operation_dispatcher.rs.pristine == 2c1a19d7e2962cf95f3ab85e99d8f51c9e1c28a848986a93f74b66d506f52fb7;
  file_sync_controller.rs = c59b49f5... (matches report).
  NOT DONE by me: re-running the neutered-gate red in a scratch copy. A scratch tree needs a cold rebuild
  (cargo fingerprints are path-absolute) and /System/Volumes/Data is at 92% (307Gi free) with 3+ other lanes
  holding the build slot. Red accepted on artifact evidence, not reproduced independently.

C2 walk gone — CONFIRMED with one overstatement.
  read_only_format_gate.rs: struct is {documents: Arc<ReadOnlyDocuments>, bus} — no Injector, no BlockReader,
  no nearest_page_ancestor. operation_dispatcher.rs:527-549 enforce_write_tier has no BlockRowMemo (two
  authority.refusal_for calls for "id"/"parent_id"). rg nearest_page_ancestor: zero hits in
  write_tier_refusal/editable_field_any paths. block_cell_registry.rs:106-119 short-circuits on
  any_read_only_documents() then one lookup.
  ReadOnlyDocuments (holon-core/src/write_tier_gate.rs) holds member_of: HashMap<EntityUri,EntityUri>;
  persisted by ONE UPDATE (turso_seams.rs:480 "UPDATE file SET content_hash = ?, read_only_blocks = ? WHERE id = ?")
  and ONE read (turso_seams.rs:377 SELECT id, content_hash, read_only_blocks FROM file). No Vec<String> bypass:
  PersistedFileState.read_only_blocks is Vec<EntityUri>, JSON parsed fail-loud at the boundary.
  OVERSTATEMENT: "the ReadOnlyMembers type makes it impossible to record membership without the file's own
  block list" is not enforced at the registry API. ReadOnlyDocuments::record is `pub` and takes
  `members: &[EntityUri]`; `ReadOnlyMembers` is a PRIVATE enum inside holon-filesystem/src/file_sync_controller.rs:610
  and `ReadOnlyMembers::Declared(&[])` is representable. The guarantee is a controller convention, not a type.

C3 fast-path behaviour — CONFIRMED for the code, REFUTED for "the test that pins it".
  Branch found: file_sync_controller.rs:2928-2947 — members from persisted_read_only_blocks; when empty AND
  read_only_docs.is_some() AND is_read_only_path(path) it logs tracing::error! ("carries no `read_only_blocks`")
  and falls THROUGH to the full ingest (no walk, no unregistered blocks). Correct as described.
  REFUTATION of the pinning claim: the branch is UNREACHABLE, so no test exercises it. The skip is gated at
  file_sync_controller.rs:2888-2894 on `disk_root = adapter.doc_id_from_content(&disk_content)`, and
  CookFormatAdapter::doc_id_from_content returns None (crates/holon-kitchen/src/file_format.rs:163-167); cooklang
  is the only ReadOnly adapter in the production registry.
  EVIDENCE I produced: `cargo nextest run -p holon-integration-tests --test cook_vault_ingest --no-capture`
  (log scratchpad/verify-readonly/cook.log) — the string "carries no `read_only_blocks`" appears ZERO times
  across all 11 tests, including a_file_row_missing_its_membership_is_rewritten_by_the_next_boot. That test
  passes because the fast path never runs, not because the branch fired.
  The lane report contradicts itself: "Pinned by a_file_row_missing_its_membership_is_rewritten_by_the_next_boot"
  (report line ~228) vs "A second probe (membership loaded as empty at the skip) stayed GREEN ... with no doc id
  the skip never runs" (report line ~296). The second statement is the true one.
  Also confirmed: after that test's boot-2 (which re-ingests) a .cook block IS still refused — the test's own
  assertion, green in my run.

C4 budget — CONFIRMED.
  `bash ~/.claude/skills/orchestrator/scripts/with-build-slot.sh just hand-authored` →
  `test result: ok. 9 passed; 0 failed`.
  DeleteBackward line, verbatim from target/gate-logs/pbt-hand-authored.log:
    [inv-sql-budget] DeleteBackward: reads=12 (dedup 8)/5 writes=0/0 ddl=0/0 tol=5
  dedup 8 <= expected 5 + tol 5 = 10. No budget EXPECTATION raised: transition_budgets.rs and
  transitions/delete_backward.rs are NOT in the diff (jj status, 38 files). The only new budget is
  AttemptReadOnlyEdit's own (tolerance 32), a new transition.
  Engagement: inv-read-only-home-refuses-writes = 5/5 and 4/4 per case in the same log.

C5 gates — CONFIRMED except keystone-smoke (did not finish in window).
  cargo check --workspace --all-targets → EXIT=0, "Finished dev profile ... in 8m 22s" (warnings only).
  cargo nextest run -p holon-loro → "367 tests run: 367 passed, 3 skipped".
  cargo nextest run -p holon-integration-tests --test cook_vault_ingest → "11 tests run: 11 passed".
    (NOTE: the SAME command with --no-capture failed 1/11,
     re_ingesting_a_recipe_replaces_its_rows_and_keeps_ingredient_ids, with "Database error: Actor channel
     closed" cascades. Load/serialization flake under a saturated machine, not a lane defect — it passes
     in the normal run. Worth a bugfunnel look if it recurs.)
  featuremap.py check → "docs/Architecture/FeatureMap.md is up to date". bugfunnel.py check → "655 entries, 0 problems".
  just keystone-smoke: STARTED (target/gate-logs/pbt-general.log, 1.08MB, 7 engagement summaries, no
  "test result:" after ~50 min wall) — the box is saturated by 3+ other lanes' cargo runs. NOT verified by me;
  the lane's own lane-logs/smoke-final.log shows "test result: ok. 4 passed; 0 failed".
  The shopping_pull_mock red was not re-run (out of my window); still needs the orchestrator's main-baseline check.

C6 latent hole — CONFIRMED.
  (a) Only OrgmodeSyncProvider emits file rows (crates/holon-orgmode/src/orgmode_sync_provider.rs:342,
      relation_name "file"; its scan is .org-only) — no other .rs writes the file relation.
  (b) CookFormatAdapter::doc_id_from_content returns None — crates/holon-kitchen/src/file_format.rs:163-167.
  persist_file_projection does NOT exist in this tree (rg: 0 hits) — lowcode-inc3 only.
  Cross-lane answer: if lowcode-inc3 lands file rows for .cook, the skip STILL will not fire, because it also
  needs doc_id_from_content != None, which cooklang does not supply. So the branch stays dead until cooklang
  gains a doc id. The real hazard is the shape of that UPSERT: an INSERT ... ON CONFLICT DO UPDATE that sets
  only its own columns is SAFE (read_only_blocks/content_hash stay standing, as
  crates/holon-filesystem/src/sync_ports.rs:131 assumes); an INSERT OR REPLACE would NULL both and silently
  make an authoritative file's blocks editable on the next boot. That is the thing to check at weave time.

## OVERALL: CONFIRMED with one refuted sub-claim (C3's pinning) and one overstatement (C2's type guarantee).
The behaviour under all five substantive claims holds; the two defects are documentation/coverage claims, not
runtime bugs.

## Defects (evidence only, no fixes)
D1. lane-report-readonly-invariant.md claims the fast-path ERROR branch is "Pinned by
    a_file_row_missing_its_membership_is_rewritten_by_the_next_boot". It is not: the ERROR string never appears
    in a full --no-capture run of that file. The report elsewhere states the true reason. The branch has NO
    covering test and cannot get one until a ReadOnly adapter supplies a doc id.
D2. C2's "impossible to record membership without the file's own block list" is not a type guarantee.
    holon-core/src/write_tier_gate.rs `pub fn record(&self, doc_id, format, path, members: &[EntityUri])`
    accepts `&[]`; ReadOnlyMembers is private to holon-filesystem.
D3. docs/Testing/bugfunnel/entries/2026-09-03-read-only-format-blocks-accept-edits-that-are-discarded.md:140
    still ends "Open; see the lane report" for the inv-sql-budget finding the same rev fixes. Entry
    status: PARTIAL. Stale within its own commit.

## Gaps
G1. The neutered-gate red was not independently reproduced (disk 92% + build-slot contention). Artifact-verified only.
G2. just keystone-smoke did not complete in my window.
G3. Not covered by the gate: OpOrigin::Sync writes are exempt from enforce_write_tier
    (operation_dispatcher.rs:536), so a peer could add a block under a read-only document root; that block is
    absent from member_of and therefore editable. Model.md 14's "membership needs no invalidation" argument
    covers store-origin writes only. Worth a ruling before pairing meets .cook vaults.

## Rev 3 — fresh-context adversarial verify (2026-09-08)

Workspace /Users/martin/Workspaces/pkm/holon/.claude/worktrees/readonly-invariant (pwd printed on every call);
@- = 830d794f878f. Read-and-run only; no tree edit, no jj/git write. Scratch logs under
/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/verify-readonly3/.

C1 ReadOnlyMembers type — CONFIRMED (closes D2).
  crates/holon-core/src/write_tier_gate.rs:39 `pub struct ReadOnlyMembers(Vec<EntityUri>)`, exported at
  crates/holon-core/src/lib.rs:70. Exactly two constructors: from_parse (:44, structurally non-empty —
  document_uri + parse.document.id + blocks) and from_persisted_row (:62, Err on empty). record (:140) takes
  &ReadOnlyMembers. HomeMembership{Declared(ReadOnlyMembers),Untouched} at file_sync_controller.rs:611.
  `rg 'record\(' crates` classified: the ONLY ReadOnlyDocuments::record call sites are
  file_sync_controller.rs:2082 (prod) and tests at write_tier_gate.rs:301/334/414 + operation_dispatcher.rs:1847.
  Every other hit is an unrelated symbol (tracing Visit::record*, Span::record, HistoryStore::record_batch,
  pairing_swap::{read,write}_record, BoundsRegistry/DispatchJournal/LatencySlo::record, ...). No `record(&[])`
  and no `Declared(&[])` anywhere.
  Extra probe I ran (not claimed): the `?` on from_persisted_row at file_sync_controller.rs:2942 cannot abort a
  boot on a damaged row — the loader never inserts an empty vec (file_sync_controller.rs:1322
  `if !row.read_only_blocks.is_empty()`), so the empty case reaches the ERROR branch, not the Err.
  My run: cargo nextest -p holon-core -E 'test(write_tier)…' → 8 tests run: 8 passed (verify-readonly3/core.log),
  incl. a_persisted_row_without_blocks_is_not_a_membership.

C2 fast-path pin is real — CONFIRMED (closes D1).
  crates/holon-orgmode/tests/ingest_contract.rs:868
  a_read_only_row_without_its_membership_refuses_the_cold_boot_skip exists and passes.
  MY RUN: cargo nextest run -p holon-orgmode --features holon-orgmode/di --test ingest_contract --no-capture →
  `10 tests run: 10 passed` (verify-readonly3/ingest.log). (Plain `-p holon-orgmode --test ingest_contract` does
  NOT compile — the crate needs holon-orgmode/di; note for anyone re-running.)
  The ERROR string appears 0× on stdout, and that is CORRECT here, not the rev-2 defect: the test installs an
  ErrorCapture layer (ingest_contract.rs:869-870) and asserts cap.mentions("carries no `read_only_blocks`") == 1
  at boot 2 (:903) and still == 1 after boot 3 (:932) — i.e. exactly once, asserted in-process. Both assertions
  hold in my green run. Anti-vacuity control is real: boot 3 asserts reader.blocks stays EMPTY (:937), which only
  holds if the skip actually fired, and asserts refusal_for_block on a FRESH registry from v.reboot() (:483-498).
  FixtureAdapter::with_embedded_doc_id (:93) feeds doc_id_from_content (:194) — that is what makes the skip
  reachable. Cook's adapter is UNTOUCHED: crates/holon-kitchen/src/file_format.rs:163-167 still returns None.
  ingest_contract.rs sha256 = ac32137671a10e17c1ac925a79ef29572c36020bcc4f20fe511ded342c2cbeee — matches the
  claimed restore. NOT reproduced by me: the red with .with_embedded_doc_id removed (would require editing a tree
  file, which this pass is barred from); accepted on lane-logs/rev3-red-nodocid.log + the in-test control above.

C3 sync-origin decision — CONFIRMED as implemented; the refutation the brief asked for HOLDS in part.
  ReadOnlyDocuments::adopt at write_tier_gate.rs:197 (separate `adopted` map, :98-101; consulted by
  refusal_for_block :235; cleared by forget :193; survives record, which only retains over member_of :142).
  WriteTierAuthority::adopt_sync_import at :274; dispatcher call at
  crates/holon/src/api/operation_dispatcher.rs:552 with a WARN.
  MY RUNS: holon-core a_block_imported_under_a_read_only_document_earns_the_refusal and
  an_imported_block_survives_a_re_ingest_and_ends_with_the_document → PASS (core.log);
  cargo nextest -p holon --lib -E 'test(a_sync_import_under_a_read_only_root_is_adopted_not_left_editable)' →
  `1 test run: 1 passed` (verify-readonly3/disp.log).
  REFUTATION RESULT (asked for): there is NO production OpOrigin::Sync emitter. `rg 'OpOrigin::Sync' crates`
  yields only crates/holon/src/api/operation_dispatcher.rs:545 (the guard itself) + :1870 (its own test),
  provenance.rs:92/194 (a match arm and a unit test), operation_engine.rs:489/2690 (match arms),
  trust.rs:40, operation_engine.rs (api) :68 — every remaining hit is under crates/holon/tests/.
  No prod `execute_operation_with_origin(..., OpOrigin::Sync)` call site exists. The Loro import leg does not
  reach the dispatcher with Sync either (crates/holon-loro/src/loro_share_backend.rs:805/842/932/2074/2084 call
  execute_operation without an origin). So the adoption fix is, today, reachable ONLY from the dispatcher path
  nothing in production drives — i.e. dead code — exactly as the hole it closes was unreachable.
  The entry does disclose this ("Not yet reachable in production — no production site dispatches with
  OpOrigin::Sync today"), so FIXED is defensible as "the latent hole is closed", but see D4 below.
  Reboot residual: DISCLOSED IN THE ENTRY ONLY. It is SILENT at runtime — the WARN fires at adopt time
  (operation_dispatcher.rs:553), nothing logs or banners at the next boot when the adoption is lost. No
  banner, no log, no degraded signal.
  Entry docs/Testing/bugfunnel/entries/2026-09-08-a-synced-block-under-a-read-only-document-stays-editable.md
  exists, gap COVERAGE, status FIXED, names the residual and the three covering tests. D3 fix confirmed:
  2026-09-03-read-only-format-blocks-accept-edits-that-are-discarded.md no longer ends "Open; see the lane
  report" for inv-sql-budget.

C4 weave note — CONFIRMED, with the reconciliation stated more sharply than the report does.
  Readonly leg (this tree): crates/holon-app/src/turso_seams.rs:457-480, ONE
  `UPDATE file SET content_hash = ?, read_only_blocks = ? WHERE id = ?` — update-only, never inserts.
  lowcode-inc3 actual (jj file show -r 91b1501d crates/holon-app/src/turso_seams.rs:446-475):
    INSERT INTO file (id, name, parent_id, content_hash, document_id) VALUES (?, ?, ?, ?, ?)
    ON CONFLICT(id) DO UPDATE SET content_hash = excluded.content_hash, document_id = excluded.document_id
  (crates/holon-filesystem/src/sync_ports.rs:134 at that rev is only the trait's no-op default.)
  What the weave must reconcile — three concrete deltas, none of them the catastrophic one:
   (a) It is ON CONFLICT DO UPDATE, NOT `INSERT OR REPLACE`, and read_only_blocks is absent from the SET list —
       so an UPDATE over an existing row leaves read_only_blocks standing. The NULLing hazard the report warns
       about does not materialise.
   (b) It DOES write content_hash (both on INSERT and in DO UPDATE), which the readonly leg's required shape
       says to leave out entirely. That is the real collision: the INSERT path creates a row with a
       disk-matching content_hash and read_only_blocks NULL. For a read-only path that is a permanently
       degraded steady state — every boot arms the skip on the hash, finds no membership, logs the ERROR
       (file_sync_controller.rs:2952) and re-ingests the file. Safe, loud, and never self-healing unless the
       ingest's own UPDATE lands after it.
   (c) lowcode's DO UPDATE does NOT set name/parent_id, so the report's proposed shape is not a drop-in
       replacement for lowcode's either; the weave has to author one statement satisfying both legs
       (name/parent_id/document_id from the projection leg, content_hash + read_only_blocks left to
       persist_file_hash, or a VALUES('') insert as the report proposes).

C5 gates — CONFIRMED, ALL RE-RUN BY ME in this workspace (not lane logs):
  cargo check --workspace --all-targets → exit 0, `Finished dev profile`, 0 `^error` lines (check.log).
  cargo nextest run -p holon-loro → `367 tests run: 367 passed, 3 skipped` (loro.log).
  cook_vault_ingest → `11 tests run: 11 passed` (cook.log). NOTE: it lives in
  crates/holon-integration-tests/tests/cook_vault_ingest.rs, i.e. `-p holon-integration-tests`, NOT `-p holon-app`
  as C5 words it.
  just hand-authored → `test result: ok. 9 passed; 0 failed; ... 1711.31s` (hand.log); 0 occurrences of
  "exceeds expected"; `DeleteBackward: reads=12 (dedup 8)/5 ... tol=5` (8 <= 5+5) plus a second
  `reads=17 (dedup 9)/5 ... tol=5`; `PASSED case "a-write-to-a-read-only-homed-block-is-refused"`.
  just keystone-smoke → `test result: ok. 4 passed; 0 failed` (smoke.log).
  bugfunnel.py check → `656 entries, 0 problems`; featuremap.py check → up to date.

## Rev 3 OVERALL: CONFIRMED. D1, D2, D3 from revs 1-2 are closed; no new runtime defect found.

### Defects (evidence only, no fix)
D4. The C3 fix is not reachable from the leg that will actually import. `adopt_sync_import` is called from
    exactly one place, crates/holon/src/api/operation_dispatcher.rs:552, on OpOrigin::Sync — and no production
    site emits OpOrigin::Sync (grep above). The Loro merge leg, which is what pairing will actually use, calls
    execute_operation without an origin (crates/holon-loro/src/loro_share_backend.rs:805/842/932). The bugfunnel
    entry names the REBOOT residual but NOT this one: when pairing lands, an import arriving through Loro will
    bypass the adoption entirely unless that leg is routed through the dispatcher with OpOrigin::Sync. The
    entry's FIXED is accurate for the dispatcher path and overstated for "a synced block ... stays editable" as
    a whole.
D5. The reboot residual is silent. Nothing at boot discloses that an adopted block's binding was dropped — the
    only signal is the WARN at adopt time in the session that already ended. Under the project's fail-loud rule
    (visible degradation over silent), a boot that loads a read-only document while holding no record of prior
    imports discloses nothing.
D6. Cosmetic/process: `cargo nextest run -p holon-orgmode --test ingest_contract` does not compile without
    `--features holon-orgmode/di` (unresolved crate::file_sync_controller). Any weave gate that runs that target
    bare will read as a red for the wrong reason.

### Gaps
G4. I did NOT reproduce the rev-3 reds myself (nodocid, sync-exemption-restored) — both need a tree-file edit,
    which this pass is barred from. Accepted on lane-logs/rev3-red-nodocid.log + lane-logs/rev3-sync-red.log and
    on the in-test anti-vacuity controls I read and ran.
G5. The `shopping_pull_mock a_local_deletion_reaches_the_peer_as_a_del_command` red flagged in the rev-2 report
    is still unattributed against main; I did not re-run it (needs a VCS-side A/B this pass cannot do).

## Rev 4 — fresh-context adversarial verify (rebased on the integration chain)

Workspace /Users/martin/Workspaces/pkm/holon/.claude/worktrees/readonly-invariant (pwd printed on every call).
`jj log`: `@` = 0020041b9518 (change sspqtqks, marked **divergent**), `@-` = aaa5a817bd55
`sw/ingest-dupslug | fix(ingest): one filter decides what the vault contains` — the claimed base.
`jj diff --stat -r @` = 40 files, +2055/-193. Scratch logs: verify-readonly3/r4-*.log.

C1 conflicts resolved by editing — CONFIRMED.
  `grep -rn '^<<<<<<<|^>>>>>>>|^|||||||' crates docs frontends scripts` → 0 lines.
  Chain-side additions verified present alongside the lane's:
  - lowcode: `persist_file_projection` on the trait (crates/holon-filesystem/src/sync_ports.rs:148) and its
    Turso impl (crates/holon-app/src/turso_seams.rs:470); the plugin-only parser landed —
    `crates/holon-kitchen/src/file_format.rs` no longer exists (cook parsing is a plugin now), and the lane
    did NOT resurrect it.
  - plaintext-layer: `IngestOutcome::RefusedEmptyFile` (file_sync_controller.rs:344, raised :3079, handled
    :5953); DocHome present in the crate.
  - ingest-dupslug: `ClaimedId` (file_sync_controller.rs:299/306/338/360/2033), `VaultFilter`
    (crates/holon-filesystem/src/vault_filter.rs, wired in lib.rs/in_memory.rs/file_sync_controller.rs).
  Lane side survives: `persisted_read_only_blocks`, `HomeMembership`, `ReadOnlyMembers::from_parse`, and
  `read_only_members` threaded through ALL FIVE persist call sites (file_sync_controller.rs:4733, :4755,
  :4770, :4831, :4926 → persist_disk_hash_for :4949).

C2 my C4 fixed at the root — CONFIRMED, and the reasoning is sound.
  `rg PersistedFileState crates` → 0 hits: the type is DELETED, so the two-row-shapes-for-one-row defect is
  gone rather than reconciled. `FileProjection` (sync_ports.rs:28-36) gains `read_only_blocks: Vec<EntityUri>`.
  ONE statement writes the row (crates/holon-app/src/turso_seams.rs:514-517):
    INSERT INTO file (id, name, parent_id, content_hash, document_id, read_only_blocks) VALUES (?,?,?,?,?,?)
    ON CONFLICT(id) DO UPDATE SET content_hash = excluded.content_hash,
                                  document_id = excluded.document_id,
                                  read_only_blocks = excluded.read_only_blocks
  `rg 'INTO file |UPDATE file |INSERT OR REPLACE INTO file'` finds exactly this one production site (the only
  other hit is a test's deliberate `UPDATE file SET read_only_blocks = NULL` at
  crates/holon-integration-tests/tests/cook_vault_ingest.rs:954). The Loro leg stays a documented no-op
  (crates/holon-app/src/loro_seams.rs:187 — no `file` table under Loro), so "one UPSERT on both legs" is
  accurate in the sense that matters: one shape, one writer.
  ASSESSMENT of `name`/`parent_id` out of the SET list: I accept it. `file.id` is `file:<vault-relative path>`
  and both columns are derived from that same path at the call site (persist_disk_hash_for:4966-4974), so on a
  conflict they cannot legitimately differ; including them would let this leg overwrite identity fields it does
  not own. The INSERT arm still supplies them, which is what makes a plugin format's row exist at all. Writing
  hash + document + membership TOGETHER is the load-bearing part and it holds: a row can no longer carry a
  disk-matching hash with a NULL membership, which was exactly the permanent ERROR+re-ingest steady state I
  named in Rev 3's C4(b).
  RED IS REAL: lane-logs/rev4-cook-red.log, with only the column removed, fails the two double-boot tests —
  `FAIL a_file_row_missing_its_membership_is_rewritten_by_the_next_boot` and
  `FAIL a_recipe_edit_is_refused_after_a_reboot`, both printing `read_only_blocks = ""`,
  `15 tests run: 13 passed, 2 failed, 1 skipped`; green log `15 tests run: 15 passed`. Probe restore verified
  by sha256: crates/holon-app/src/turso_seams.rs = dc80d6206d49e3bbbf0165c116445f3eb52f8f5de226c0dd13f13d5aa6c9d27d,
  matching the report's claim. MY OWN RUN of that target: `15 tests run: 15 passed, 1 skipped`
  (verify-readonly3/r4-cook.log), with both double-boot tests PASS.

C3 arm_the_cold_boot_skip hand-INSERT deleted — CONFIRMED.
  crates/holon-integration-tests/tests/cook_vault_ingest.rs:815-817 is now `re_save(env, content).await;` and
  nothing else; the helper no longer inserts a `file` row, which the UPSERT now creates. Consistent with the
  claimed `UNIQUE constraint failed: file.id` had it stayed.

C4 _context deletion + FeatureMap — CONFIRMED.
  `jj diff -r @ --summary | rg _context` → nothing: the lane's diff touches no `_context` path, i.e. it accepted
  the chain's side. (The two regenerated files `_context/crates/current.json` and
  `_context/frontends/current.json` exist in the working tree but are untracked/ignored.)
  docs/Architecture/FeatureMap.md:39 reads "Its alphabet is 75 transitions; the repo declares 76 invariant ids
  plus 17 correspondence-family ids". `featuremap.py check` → "is up to date" (my run).

C5 D4/D5 disclosed — CONFIRMED.
  docs/Testing/bugfunnel/entries/2026-09-08-a-synced-block-under-a-read-only-document-stays-editable.md,
  `## Residual — two, both open and both disclosed here only`: item 1 names the Loro import leg bypassing
  adoption (`loro_share_backend.rs:808/:845/:935`, origin-less `execute_operation` defaulting to
  `OpOrigin::User`); item 2 names the reboot residual AND that its loss is silent. Both of my Rev-3 defects are
  now on the record.

### Gates I re-ran myself (semaphore-wrapped, logs in verify-readonly3/)
| gate | my result |
|---|---|
| `cargo check --workspace --all-targets` | exit 0, `Finished dev profile … in 5m 51s`, 0 `^error` lines (r4-check.log) |
| nextest `-p holon-filesystem -p holon-core -p holon-app -p holon-orgmode --features holon-orgmode/di` | `647 tests run: 647 passed (4 slow), 1 skipped`, 0 FAIL (r4-crates.log) — includes `ingest_contract a_read_only_row_without_its_membership_refuses_the_cold_boot_skip` (518/647) and the two adoption unit tests |
| `--test cook_vault_ingest` (extra, not on the brief's list) | `15 tests run: 15 passed, 1 skipped` (r4-cook.log) |
| `just loro-suite` | `16 tests run: 16 passed, 1 skipped` (r4-loro.log) |
| `just check-worker-wasm` | `test result: ok. 5 passed; 0 failed` (r4-wasm.log) |
| `bugfunnel.py check` | `… entries, 0 problems` (r4-bugfunnel.log) |
| `featuremap.py check` | `is up to date` (r4-featuremap.log) |
| `just hand-authored` | `test result: FAILED. 8 passed; 1 failed … 40.51s` (r4-hand.log) — see D7 |
| `just keystone-smoke`, `just analyze-arch` | still running under heavy machine contention at write time; addendum below |

## Rev 4 OVERALL: CONFIRMED (all five claims). One reporting defect, no runtime defect.

### Defects (evidence only, no fix)
D7. **The report's "8 of 9 cases pass" for `just hand-authored` is a misreading, and it hides that this lane's
    own keystone pin never ran.** `8 passed; 1 failed` is the Rust TEST counter for the binary, not the case
    counter. In my run (r4-hand.log) and in the lane's own lane-logs/rev4-hand4.log, exactly FIVE cases start
    and FOUR pass:
      create-block-smoke, create-block-smoke-pinned-turso-draw, editor-trailing-space-echo-adopts-baseline,
      external-write-while-focused-preserves-dirty-buffer, then
      `running case "ref-doc-0-remap-born-equal-create-under-focus"` → panic at
      crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:4047
      `[SutAppLifecycle::create_document] timeout waiting for the doc block (title "doc_0") to land in
      block_raw after writing doc_0.org`.
    The harness aborts there, so every LATER case is skipped — including
    `a-write-to-a-read-only-homed-block-is-refused`, which is the LAST entry in
    crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl and is THIS lane's own keystone
    regression pin. It is present in the jsonl and it passed at rev 3 on main (1711s run, `PASSED case`), but
    at this chain tip it is UNVERIFIED because the chain-side red masks it.
    CONFIRMED as asked: `ref-doc-0-remap-born-equal-create-under-focus` is the ONLY red observed — but only
    because the run stops there; "only red" and "everything else green" are not the same statement here.
D8. Minor, inherited by the merge: `persist_disk_hash_for` (file_sync_controller.rs:4984-4996) downgrades a
    failed `persist_file_projection` to `warn!` while having ALREADY updated `last_projection_hash` in memory.
    The consequence is benign (the row keeps its older hash+membership, so the next boot re-ingests), but it is
    an error swallowed against the project's fail-loud rule, and it is the one place a hash and its membership
    can drift apart within a session.

### Weave safety
Nothing in the diff blocks the weave on this lane's account: the row-shape collision that motivated rev 4 is
resolved at the root (one type, one statement, membership travelling with the hash), the chain's contributions
survive, and every gate I could complete is green. The open risk is D7 — the lane's own hand-authored pin
cannot be shown green at this base until the plaintext-layer `ref-doc-0-remap` red is cleared. Recommend the
weave order put that lane's fix first, or re-run `just hand-authored` with the failing case temporarily
skipped, before treating this lane's keystone coverage as proven at the chain tip.

### Gaps
G6. `just keystone-smoke` and `just analyze-arch` did not finish inside my window (six other cargo jobs on the
    box). Lane logs claim `4 passed; 0 failed` and `85 baselined … 0 new violation(s)`; NOT reproduced by me.
G7. The rev-4 reds (column removed) were not reproduced by me — they need a tree edit this pass is barred from.
    Accepted on lane-logs/rev4-cook-red.log plus the matching sha256 restore I checked myself.

### Rev 4 addendum — the two gates that were still running (supersedes G6)

`just analyze-arch` — PASS, reproduced:
  `archlint: 85 baselined violation(s) suppressed (see archlint/baseline.txt), 0 new violation(s).`
  `archlint: baseline stale - 19 entry(ies) no longer fire; run ./archlint/archlint --update-baseline …`
  (verify-readonly3/r4-arch.log). Matches the lane's claim, advisory line included.

`just keystone-smoke` — RED on my first run, GREEN on my second. Both are mine, same tree:
  run 1 (r4-smoke.log): `test result: FAILED. 3 passed; 1 failed … finished in 1097.85s`
  run 2 (r4-smoke2.log): `test result: ok. 4 passed; 0 failed … finished in 3.07s`
  The recipe is `just pbt general 1` (justfile:188) — ONE randomly-seeded case, so the two runs drew different
  inputs and the gate is stochastic, not deterministic. The lane's `2.12s / 4 passed` is a green draw like my
  run 2, not a false green.
  The draw that failed is worth recording because it is an UNREGISTERED signature. Five arms co-fire on one
  block, `block:bulk-1-0`, all saying the SUT stored a literal empty link where the reference has nothing:
    inv-blocks-match-ref/org, /block_raw, /matview: `content: sut="[[]]" ref=""`
    inv-block-content/block_raw and /sql: `ref: ""` vs `"[[]]"`
    harness.rs:1206; `lowest MEASURED failing layer: matview/SQL … NOT proven first-divergent`
  It reaches block_raw/sql/org, so it is a STORE divergence, not a projection artifact.
  docs/Testing/KeystoneKnownReds.md registers no `inv-block-content` signature at all (`grep -c inv-block-content`
  → 0) and no `[[]]` content arm; the `bulk-1-0` entries there are the sibling-ORDER family
  (`bulk-add-sibling-order-under-journals`), a different assertion. Per CLAUDE.md's rule ("anything else is a
  regression"), this needs triage rather than a shrug.
  ATTRIBUTION — not this lane, on the evidence I have: the lane's diff touches the write-tier gate, the `file`
  row shape and its persistence; it touches no content, link-mark, or `BulkExternalAdd` path, and the failing
  case involves no read-only file. I could NOT run the A/B against `aaa5a817bd55` (that needs a VCS operation
  this pass is barred from), so this is unproven either way. Recommend the orchestrator replay the draw at the
  chain tip before the weave and, if it reproduces, file it (bugfunnel + KeystoneKnownReds) against the chain
  rather than this lane.

Revised gate table line: `just analyze-arch` PASS; `just keystone-smoke` PASS on re-run, with one red draw
recorded above. Nothing here changes the Rev 4 verdict: CONFIRMED, with D7 (masked hand-authored cases) as the
weave-relevant finding and this stochastic keystone draw as a second item for the chain's triage queue.
