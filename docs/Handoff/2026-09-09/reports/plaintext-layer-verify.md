# Verify: plaintext-layer — CONFIRMED with 2 sub-claim refutations

Workspace /Users/martin/Workspaces/pkm/holon/.claude/worktrees/plaintext-layer, @- = 830d794f878f, both tree markers present. All evidence produced in this session.

## C1 — PARTIAL REFUTE
Types/routing confirmed: PageAncestor{Page,NoOwner,Broken(PageWalkBreak)} (sync_ports.rs:952-1043), DocHome{Resolved,Untracked,Unresolvable}, route_homed_block Untracked=>Drop / Unresolvable=>Recover+WARN, feed loop uses route_remove (di.rs:814). rg over all call sites: only 3 into_page() sites, all deliberate (read_only_format_gate.rs:53, file_sync_controller.rs:2637/6462); no `_ =>` default arm in any of the 3 changed files.

REFUTED sub-claim "the WARN carries the block id and the break reason": it carries id and parent_id only. DocHome::from_walk (home_authority.rs:107) does `PageAncestor::Broken(_) => DocHome::Unresolvable` — the reason is discarded at the type level and is structurally unavailable at the warn site. Worse: of the 3 breaks, ParentCycle and DepthBound log at the walk (sync_ports.rs:1026,1039) but ChainLeftTheStore (:1032) logs NOTHING anywhere. That is the likeliest Broken cause in production (a parent row not yet in the store) and it now fires a vault-wide re-render with no reason disclosed at all.

## C2 — CONFIRMED (restore) / ATTESTED (red)
Restore proof reproduced by my own shasum: di.rs 4ba09f82…, file_sync_controller.rs 01dce092… — byte-identical to the report; `rg -c RED-PROBE crates` = 0 matches. Assertion lines quoted at report L137-154. The red half I did NOT re-run: reproducing it needs edits to tree files, which my read-only mandate forbids. Green half reproduced: all 8 tests of the two new files PASS in my run.

## C3 — CONFIRMED, one sub-claim REFUTED
RefusedEmptyFile before parsing (file_sync_controller.rs:2769-2779, guard sits above the ORGSYNC_ENTER parse); poll_new_files quarantine arm keyed on `sig`=(mtime,size) at :5438-5447, so a later content write changes size and is re-decided. `disk_content.is_empty()`, not trim().
LIVE WATCHER LEG IS COVERED — not a gap: on_file_changed (:2263) funnels into ingest_file (:2298), so the same refusal applies. I checked the two pre-ingest steps that run on raw bytes first: heal_title_less_doc_root is a no-op at 0 bytes (doc_id_from_content("") => None, :1554).
REFUTED sub-claim "whitespace-only files are ingested (find the test)": no such test exists. empty_file_is_not_a_document.rs contains exactly 2 tests (:267 a_zero_byte_save_intermediate_leaves_the_documents_blocks_alone, :316 a_zero_byte_file_is_not_written_back); `rg whitespace|trim\(\)|is_empty` over holon-orgmode/tests and holon-filesystem/tests returns nothing. The behaviour is correct but unpinned — a future trim() "cleanup" would regress it silently.

## C4 — CONFIRMED, gap listed
New harness applies: empty_file_is_not_a_document.rs:116-119 removes from the store. page_rename_retires_old_file.rs:245 and ingest_data_loss_guard.rs:196 also apply. 13 neighbouring harnesses still answer Ok(())/unimplemented and would hide the same delete-clobber class; the ingest-path ones that matter: ingest_contract.rs:408, file_level_drawer_seam.rs:110, incremental_org_writeback_smoke.rs:215 and :1145, directory_companion_adoption.rs:90, idonly_title_heal.rs:85, poll_new_files_containment.rs:125, dotted_page_title_writeback.rs:136, name_chain_error_propagation.rs:205. (Also vault_path_escape.rs:182, writeback_readonly_skip.rs:169, writeback_emits_authored_link_bytes.rs:164, proposal_writes_do_not_amplify.rs:236, and the lane's own routing_applicability.rs:121 — those do not ingest, so lower risk.) None were changed by this lane.

## C5 — CONFIRMED (all three run by me)
- cargo check --workspace --all-targets: Finished dev profile in 53.84s, `grep -cE "^error"` = 0.
- cargo nextest -p holon-app -p holon-org-format -p holon-orgmode -p holon-filesystem --features di,holon-orgmode/di --no-fail-fast: `Summary [51.509s] 736 tests run: 735 passed (1 slow), 1 failed, 2 skipped`; the single FAIL is exactly holon-app::shopping_pull_mock a_local_deletion_reaches_the_peer_as_a_del_command. No other regression.
- just keystone-smoke: `test result: ok. 4 passed; 0 failed` in 0.92s.
NOT verified: that the shopping_pull_mock red is red on base 830d794f. The lane did not run the base either; its dependency-closure argument is sound but is an argument, not evidence. Orchestrator must confirm on base or add the signature before weaving.

## C6 — CONFIRMED with a correction and a gap
Rows in /Users/martin/Workspaces/pkm/holon-pkm/Projects/Holon/Plain-Text Layer.org: :ID: empty-doc-skip-watcher (L55) and :ID: routing-applicability-vs-missing (L62).
CORRECTION to the report: the `holon:crates/holon-orgmode/src/di.rs` child block is at L54 and belongs to the sibling DONE debounce row, NOT to the routing row. The routing row has no file-pointer child; its body text says only "in di.rs". The lane's file choice is still right, but the report overstates the vault's specificity.
Routing row acceptance met: "blocks under sentinel parents must not arm the fallback" -> an_untracked_block_arms_no_bulk_pass; "pinned by a unit test in holon-orgmode" -> routing_applicability.rs. Both green in my run.
GAP: the 0-byte row's done-when is a LIVE observation ("a 0-byte Journals/<date>.org produces zero 'No document found for path' lines in a 60 s log slice"). No such observation was made. That log line lives in re_render_all_tracked (file_sync_controller.rs:5594) and is already `debug!`, so at default INFO the criterion is vacuously satisfied and cannot discriminate. The criterion needs restating, or a dogfood log slice.

## Additional gap (mine, not in the lane report)
An intentionally-emptied .org file now never converges: it is refused, quarantined on (mtime,size), and the only disclosure is one INFO line. There is no path by which a user who deliberately cleared a file sees Holon act or warn. Deliberate per the code comment, but undisclosed to the user and unbounded in time.

## Verdict
CONFIRMED overall — both G1 rows are really implemented, all three gates reproduce green, and the suspected live-watcher gap does not exist. Two sub-claims REFUTED: the WARN does not carry the break reason (and ChainLeftTheStore is silent everywhere), and the claimed whitespace-only test does not exist.

---

# Rev 2 — DELTA verification

Same workspace, `@-` still 830d794f878f, rev 2 uncommitted in `@`.

## D1 — reason reaches the WARN: CONFIRMED
`DocHome::Unresolvable(UnresolvedHome)` with `Walk(PageWalkBreak) | BatchPassGap` (home_authority.rs:100-119). `from_walk` now binds the reason (`Broken(why) => Unresolvable(Walk(why))`, :132); `locate_batch` uses `BatchPassGap` (:487).
`rg -n "Broken\(_\)" crates` returns exactly ONE hit — `sync_ports.rs:1009`, inside `PageAncestor::into_page()`. That is the documented single collapse point for callers that only want "which page", not a routing decision. **No collapse remains on any routing leg.**
WARN text format (di.rs:979 and :1046) both interpolate `{why}`; `PageWalkBreak`'s Display (sync_ports.rs:973-984) renders the repair, e.g. ChainLeftTheStore => "a row on its parent chain is not in the store". `route_remove` is no longer silent — rev 1's silent departure leg now WARNs (di.rs:1043-1052). `ChainLeftTheStore` WARNs at the walk itself.
Both of my rev-1 refutations are addressed, and the tests assert the log TEXT rather than the enum: `routing_applicability.rs:313 the_recovery_warn_names_the_break_that_caused_it` greps the captured WARN for "not in the store"; `:343 a_chain_that_leaves_the_store_is_disclosed_at_the_walk` pins the previously-silent break. That is the grep-the-format check, done inside the suite where it cannot rot.

## D2 — whitespace test: CONFIRMED
`empty_file_is_not_a_document.rs:395 a_whitespace_only_file_is_still_ingested` writes "\n   \n" and asserts the document comes into being. My rev-1 refutation is closed.

## D3 — emptied-file disclosure: CONFIRMED by reading; test asserts exactly the delta
`EmptyFileWatch{first_seen_millis, disclosed}` (file_sync_controller.rs:341-348), `EMPTY_FILE_GRACE_MILLIS`, `disclose_persistent_emptiness` (:5371-5422). Once-only is enforced by the `disclosed` flag, and the function re-stats before disclosing so a file whose bytes landed clears instead of firing.
I probed the obvious refutation — that the `(mtime,size)` ingest quarantine would short-circuit the 5 s re-check and the disclosure would never fire. **It does not**: the call at :5450 is placed BEFORE both the `last_projection` tracked-file skip and the quarantine check, with a comment saying exactly why. Refutation closed.
`empty_file_is_not_a_document.rs:427 a_file_that_stays_empty_is_disclosed_as_degraded` is the test I would have written: it polls once inside the grace window and asserts NO disclosure, jumps the injected `Clock` to 60 s, polls TWICE, asserts `emptied.len() == 1` (the once-only probe), asserts `v.block_ids() == before` (the document is KEPT, not deleted), then restores content and asserts the all-clear fires.
Bus + toast wiring: `ShareDegradedReason::VaultFileEmptied` + `VAULT_FILE_EMPTIED` key (degraded_signal_bus.rs), cleared alongside `VAULT_INGEST_FAILED` in `ingest_recovered` (loro_seams.rs:659-670), amber toast in share_ui.rs. The `WritebackDisclosure::vault_file_emptied` trait method has NO default body — every implementor is forced to decide (2 existing harnesses updated).

## D4 — the 16 fakes: CONFIRMED
Tabled at file:line in the report (L452-470). I independently re-derived the list; it matches, including the 9 ingest-path ones in the same order. Handed off, not fixed — correct call.

## Scope
`crates/` touched: holon-app (loro_seams, move_guard, read_only_format_gate, rehome_entity), holon-filesystem, holon-loro (degraded_signal_bus), holon-orgmode. Nothing outside those. NOTE for the orchestrator: rev 2 GREW the blast radius versus rev 1 — holon-loro and `frontends/gpui/src/share_ui.rs` are new surface, added because the disclosure is now user-facing. Justified, but it is a bigger weave than rev 1. Also `scripts-lane/r2-probe.sh` and `r2-run.sh` are new TRACKED files; `scripts-lane/` already held 14 others from earlier lanes, so this follows the established pattern — flagging only so it is a deliberate choice.

## Gates — IN FLIGHT at time of writing
I re-ran the 4-crate nextest gate under `with-build-slot.sh` (script file, log `/private/tmp/.../verify-plaintext/gate-rev2.log`). It is QUEUED behind other lanes on the contended 4-slot semaphore: log 0 bytes, `pgrep -f verify-plaintext/gate.sh` = 3 — the known "0-byte semaphored log means running, not failed" signature, not a dead run. Rev 1's identical gate reproduced cleanly earlier this session (735/736, shopping bomb only), and `cargo check --workspace --all-targets` was 0 errors.

## Rev 2 verdict
CONFIRMED on every delta claim I could verify by reading and by the suite's own assertions. All four rev-1 defects are genuinely fixed, and the two refutations I raised are closed by code, not by wording. The one outstanding item is my own re-run of the rev-2 gate, still queued on the build semaphore; the lane's claimed 740/739 is not yet independently reproduced.
Unchanged carry-over from rev 1: the shopping_pull_mock red is still not proven red on base 830d794f — confirm on base or add the signature before weaving.

---

# Rev 3 — DELTA verification

Workspace as before. NOTE: while I worked, the parallel weave in `_sw_integ` REBASED this commit under me — change `xqrttnyq` moved from `1268f8da46bf` to `f972dd37de64` and my working copy auto-updated to a fresh empty `@` on top of it. I did not touch `_sw_integ`. Rev-3 content verified present in the tree (`holds_content_for` at file_sync_controller.rs:2697, both new tests present); `jj diff -r xqrttnyq --stat` = the claimed 2 files, +86/-6.

## C2 — prod-faithfulness: CONFIRMED with one nuance
`test_environment.rs:389` does inject `Arc::new(holon_filesystem::InMemoryFileSystem::new())` as the org FS, so the keystone drives the same FS implementation as the harness. `render_document_header` (holon-org-format/src/models.rs:552-559) does deliberately emit NO `#+TITLE:` for a title-less doc-root, with the stated reason that a synthetic one would break `parse(render(doc)) == doc`.
NUANCE: that alone does not make the file 0 bytes. Four lines above (:548-550) the same function emits `#+ID: <id>` whenever `doc_block.id.is_block() && (!drawer_carries_id || authored_id_keyword)`. So a page renders to zero bytes only when it ALSO takes no `#+ID:` line — a `doc:`-scheme root, or one whose drawer carries the id. The claim "an empty page's canonical on-disk form IS a 0-byte file" is therefore true for a subclass, not universally. It does not weaken the fix (the keystone red is empirical proof that the real pipeline does produce such a file), but the report states it more broadly than the code supports.

## C3 — the fallback and the restart: MECHANISM CONFIRMED, DURABILITY UNTESTABLE IN THIS HARNESS
I traced the whole leg rather than trusting the comment:
- `initialize()` (file_sync_controller.rs:1328-1358) calls `block_reader.load_file_projections()` and inserts EVERY returned row into `last_projection_hash`. Not filtered to this session, not filtered by format — every persisted `file` row with a resolvable in-vault path.
- The write side is `persist_disk_hash_for` (:4638), which inserts in memory AND calls `persist_file_projection`. Its five call sites (:4429, :4451, :4466, :4527, :4622) are ALL on the ingest path — so a file that was only ever INGESTED from disk, never written back, does persist its hash. My first hypothesis (that only Holon-written files would be covered, re-opening the hole after restart for read-only files) is REFUTED.
So claim (3) is structurally sound and the answer to "every file or only this session's" is: every file with a persisted `file.content_hash` row, which ingest creates.

THE PROBE THE COORDINATOR ASKED FOR CANNOT BE WRITTEN AGAINST THIS HARNESS TODAY, and the reason is itself a finding. Both durability methods on the reader trait have SILENTLY-SUCCEEDING DEFAULT BODIES (sync_ports.rs:122-124 `load_file_projections` -> `Ok(Vec::new())`; :134-142 `persist_file_projection` -> `Ok(())`), and the test harness's `StoreReader` overrides NEITHER. So in every test in `empty_file_is_not_a_document.rs` the persist leg is a no-op and the load leg returns nothing.
Consequence: a cold-boot probe written today (seed the hash, build a second controller, `initialize()`, present a 0-byte file, assert the blocks survive) would go RED — but red because the harness persists nothing, indistinguishable from red because production is broken. It cannot ATTRIBUTE. That is why the durability is argued, not tested.
MISSING SEAM, exactly: `StoreReader` must override `load_file_projections`/`persist_file_projection` against a map held in the shared `Store`, so a second controller constructed over the same store observes what the first persisted. Then the probe is: ingest the fixture with content (first controller), drop it, build a second controller over the same store, `initialize()`, truncate the file to 0 bytes, `on_file_changed`, assert `block_ids()` unchanged AND the outcome is `RefusedEmptyFile`. A companion negative — seed NO hash, present a 0-byte file, assert the page IS created — pins that the fallback is what decided it.
This is the same fail-quiet shape the lane itself called out for `delete_in_tree` in rev 1, recurring on the very mechanism rev 3's durability claim rests on. Note the contrast: `WritebackDisclosure::vault_file_emptied` was deliberately given NO default so implementors must decide; these two were not.

## Does the new condition re-open the ORIGINAL defect? NO, with one residual
Reasoning from `holds_content_for` at the instant of the 0-byte observation:
- Same session, file had content: `last_projection` holds those non-empty bytes (set on the ingest path at :2994/:4582 and on every write-back), so the first branch returns true and the refusal fires. Protected — and pinned by `a_zero_byte_file_at_a_path_with_content_is_still_refused`.
- After a restart: `last_projection` is empty, `last_projection_hash` carries the persisted hash, and the comparison is `*recorded != self.projection_hash("")` — a file that had content hashes to something other than the empty render, so it returns true and the refusal fires. Protected, but by the UNTESTED branch above.
- Path Holon holds nothing for: no document exists there, so there are no blocks to delete. Correctly not refused.
RESIDUAL: `persist_disk_hash_for` is best-effort by design — a `persist_file_projection` failure only warns ("next boot will re-ingest", :4667). A file whose hash failed to persist has no row, so after a restart `holds_content_for` is false and a 0-byte mid-save observation WOULD delete its blocks. Narrow (needs a prior disclosed persist failure plus a save in flight across the restart) and pre-existing in shape, but it is the one path where the original defect survives rev 3.

## Interaction with rev 2's disclosure — checked, correct
`empty_since` is now entered only inside the refusal branch, so a 0-byte file at a path holding nothing never enters the emptied-file watch and is never disclosed as stale. That is right: nothing there is stale. A file with content that is emptied stays refused (its `last_projection` keeps the old non-empty bytes, so `holds_content_for` remains true), so it still reaches the grace period and discloses exactly once. Rev 3 does not regress rev 2.

## C1 — the keystone red was deterministic: CONFIRMED (lane logs, internally consistent)
`ref-doc-0-remap-born-equal-create-under-focus` is present in `crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`. Citing lane logs by name only: `r2-ha1-45113.log`, `r2-ha2-15887.log`, `r2-ha3-17845.log` each end `test result: FAILED. 8 passed; 1 failed` and each names that case — 3/3, deterministic, not load-dependent. `r2-ha4-32978.log` ends `test result: ok. 9 passed; 0 failed` after the fix. Consistent with the claim.

## Gates — my own re-run OUTSTANDING
`rev3.sh` (hand-authored, then `-p holon-filesystem -p holon-orgmode`, then the two named tests) is running under the semaphore; log `/private/tmp/.../verify-plaintext/rev3-gate.log`. Each stage pipes into `tail`, so the log stays 0 bytes until a stage completes; the run is alive (script present, rustc/nextest active). Note the lane's own ha4 shows `hand-authored` alone taking 768s green, so this is a long run.
PROCESS FLAG FOR THE ORCHESTRATOR: this is the SECOND consecutive rev whose gate I could not reproduce within the session — rev 2's `gate-rev2.log` also never produced output and its process is gone. Independent gate reproduction is therefore outstanding for BOTH rev 2 and rev 3; only rev 1's gate was reproduced by me (735/736, shopping bomb only) plus `cargo check --workspace --all-targets` at 0 errors.

## Rev 3 verdict
CONFIRMED on every claim I could verify by code-trace and by the lane's logs. The fix is correctly scoped: it does NOT re-open the original clobber, because `holds_content_for` consults the session projection first and the persisted hash second, and both are populated on the ingest path. My hypothesis that only Holon-written files would be covered is REFUTED by the five `persist_disk_hash_for` call sites.
Two things the orchestrator should carry:
1. The cold-boot durability branch is UNTESTABLE in this harness because `load_file_projections` and `persist_file_projection` have silently-succeeding defaults the harness does not override. Missing seam described above. This is the same fail-quiet shape the lane fixed for `delete_in_tree`.
2. Residual: a file whose `persist_file_projection` failed (best-effort, warn-only) loses the protection across a restart.

## Gates — REPRODUCED BY ME (supersedes the "outstanding" note above)
Log `/private/tmp/.../verify-plaintext/rev3-gate.log`, run under `with-build-slot.sh` from a script file, exit 0.
- A `just hand-authored`: `test result: ok. 9 passed; 0 failed; 0 ignored` in 984.85s. `EXIT_A=0`. Claim of 9/9 reproduced.
- B `cargo nextest run -p holon-filesystem -p holon-orgmode --features holon-orgmode/di --no-fail-fast`: `Summary [32.991s] 312 tests run: 312 passed (1 slow), 0 skipped`. `EXIT_B=0`.
- C the two named rev-3 tests: `Summary [0.042s] 2 tests run: 2 passed, 213 skipped`. `EXIT_C=0`.

COUNT DISCREPANCY, benign: the lane claims 324/324 for gate B; I measured 312/312. Not a failure — 0 failed either way — and the flags were right (`holon-filesystem` has no `[features]` section at all, `holon-orgmode` has only `di`). The cause is that I measured on the REBASED commit (the weave moved `xqrttnyq` onto chain tip `aaa5a817bd55`), so the base under the lane's two crates is not the base the lane measured on. The orchestrator should treat 324 as a pre-rebase number.
I confirmed the rebase did NOT drop rev-2 work: `empty_file_is_not_a_document.rs` + `routing_applicability.rs` still hold 15 async tests, including `a_file_that_stays_empty_is_disclosed_as_degraded`, `the_recovery_warn_names_the_break_that_caused_it` and `a_whitespace_only_file_is_still_ingested`, and they ran in gate B.

This also retires the process flag above for rev 3. Rev 2's own gate remains unreproduced by me, but gate B here covers holon-orgmode and holon-filesystem at the rebased tip, which is where all of rev 2's tests live — all green.
