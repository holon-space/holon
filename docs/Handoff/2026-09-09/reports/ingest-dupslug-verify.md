# Verify — ingest-dupslug (fresh-context adversarial)

Workspace `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/ingest-dupslug`, `@-` = 830d794f878f, tree assert OK. All commands run from that abs path.

## Verdicts

C1 CONFIRMED (with two named untested edges). All 88 `read_dir|walkdir|WalkBuilder|notify::|EventKind` hits classified: only `fs_port.rs:222 walk_directory` (boot), `file_watcher.rs is_vault_relevant` (live), `in_memory.rs:299` (harness) enumerate/react to vault files. `change_source.rs` is the raw notify port, filtered downstream by design; `shared_snapshot_store.rs`, `plugin-host/adapter.rs`, `properties_bag_write.rs` read non-vault dirs. Rule = "any dot-prefixed Normal component BELOW root", dirs AND files alike. Tested: dot-dirs (.claude/.jj/.git/.obsidian/.logseq), dot-FILE leaf (`.draft.org` — vault_path test + `ignore`'s hidden(true) agree), hidden ROOT (I reproduced: `rg --files` under a `.pkmroot` still lists `Projects/A.org`). UNTESTED: symlinks. Reproduced in scratch — `ln -s <root>/.claude/worktrees/x <root>/mirror`; `rg --files` (walk proxy, follow_links=false) omits `mirror/A.org`, but `hidden_vault_segment(root, root/mirror/A.org)` → None, so the watcher would accept it. Real-world risk low (FSEvents does not follow symlinks either), but the legs are not provably identical on that axis.

C2 CONFIRMED-with-GAP. Both legs are wired through ONE loop (`di.rs:1043 run_file_sync_controller` → `VaultFileWatcher::new` at 1075 + `scan_directory`), so the harness genuinely drives the watcher filter, and the test writes LIVE_COPY after `start_app` and waits on the seq. But the MEASURED red (agent 2) reverted only the `in_memory.rs` scan filter — that pins the SCAN leg end-to-end. The watcher leg is pinned only at unit level (`file_watcher::tests::a_hidden_nested_vault_copy_is_never_ingested`, real fs, and the measured-red parity test). GAP: no measured end-to-end red for the watcher leg alone. My attempt to prove it by log (`RUST_LOG=... --no-capture`) was inconclusive — the org_suite harness installs no subscriber, zero DEBUG lines emitted.

C3 CONFIRMED. `file_sync_controller.rs:2819` doc-level refusal exists (`live_claimant_of` 1885, `disclose_duplicate_doc_id` 1981, `DUPLICATE_ID_SITE` 287). Entry `2026-09-09-block-slug-claimed-by-another-file-merges-with-no-refusal.md` is OPEN, states BOTH policy options (drop whole file vs. drop colliding subtree) with each one's cost, and says "Escalate before implementing". No `block_home` / block-level refusal exists in the diff — verified by grep.

C4 CONFIRMED with a wording defect. `bugfunnel.py check` → `658 entries, 0 problems` (reproduced). Entry 1's byte-identity claim IS corrected: it now reads "Byte-identity is deliberately NOT the oracle", which matches the test (asserts `block:dupslug-copy-done` absent from `block_raw` and from the live bytes, plus `dupslug-live-second` retained). DEFECT in the claim, not the tree: "three entries name covering tests that exist and pass" is false — only entry 1 names covering tests; entries 2 and 3 are OPEN and name MISSING pieces (entry 2 says "Not yet reproduced end to end").

C5 CONFIRMED, all four gates reproduced by me via `with-build-slot.sh` + script files.
- `cargo check --workspace --all-targets` → `Finished dev … in 7m 55s`, EXIT=0. No transient E0432; agent 2's flake did not recur.
- `cargo nextest run --no-fail-fast -p holon-filesystem -p holon-orgmode --features holon-orgmode/di` → `305 tests run: 305 passed`. All 7 new tests PASS.
- `cargo nextest run --no-fail-fast -p holon-integration-tests --test org_suite nested_vault_copy_dup_slug` → `1 passed, 39 skipped`.
- `cargo nextest run --no-fail-fast -p holon-org-format -p holon-app` → `431 tests run: 430 passed, 1 failed`; the sole red is `holon-app::shopping_pull_mock a_local_deletion_reaches_the_peer_as_a_del_command` — the briefed time bomb, untouched by this diff.
- `just keystone-smoke` → `test result: ok. 4 passed; 0 failed`, KSEXIT=0.
Logs: `/private/tmp/…/scratchpad/verify-dupslug/g{1,2,3,4}-*.log`.

C6 REFUTED-AS-STATED (numbers), mechanism CONFIRMED. Read-only in `/Users/martin/Workspaces/pkm/holon-pkm`: `rg --files -g 'Now.org'` → **1**; `rg --files --hidden -g 'Now.org'` → **12**, not 13. 11 copies under `.claude/worktrees/agent-*/Projects/Holon/Now.org` + the live file. Vault never opened in Holon. The defect shape is exactly as claimed; only the count is off by one (a workspace likely disappeared since the measurement).

## Overall: CONFIRMED

The fix is real, the three legs share one rule, every gate reproduces, and the sole red is the briefed time bomb.

## Defects (evidence only, not fixed)

1. Lane-report count "13" does not reproduce; today it is 12 (`rg --files --hidden -g 'Now.org'` in holon-pkm).
2. Claim C4's "three entries name covering tests" overstates: entries 2 and 3 are OPEN with no covering test.

## Gaps

- Watcher leg has no MEASURED end-to-end red (unit-level only). Cheapest close: revert only `file_watcher.rs`'s `hidden_vault_segment` call and re-run the org_suite test.
- Symlink axis is untested and the two legs are not provably identical there (walk excludes by not following; the rule has no symlink concept).
- The legs still diverge on GITIGNORE (pre-existing, outside the claim, uncovered by the parity test whose corpus has no `.gitignore`): `walk_directory` uses `WalkBuilder` defaults (`parents`, nested `.gitignore`, `.ignore`, `git_exclude`, `require_git=true`), while `file_watcher::build_gitignore` reads ONLY `<root>/.gitignore`, unconditionally. In a non-git vault the watcher drops gitignored files the boot walk ingests — the same class of bug, one axis over.
- `hidden_vault_segment`'s `strip_prefix(root).unwrap_or(path)` fails CLOSED on a root mismatch (judges the whole path): with a `tempfile` root named `.tmpXXXX`, any non-canonical path that fails to strip would be silently dropped as hidden. Not reproduced; noted as a silent-drop hazard.
- Full `-p holon-integration-tests` suite not run (25 pre-existing reds on main, never gated); no harness test writes org fixtures under a dot-path, so the `in_memory.rs` behaviour change has no other consumer I could find.

---

# Rev 3/4 delta re-verify — CONFIRMED

`pwd` for every command: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/ingest-dupslug`; `@-` still 830d794f878f; `VaultFilter` and `block_home` both present; diff now 11 files / 1252 insertions.

## (d) Gates — reproduced
- `cargo nextest run --no-fail-fast -p holon-filesystem -p holon-orgmode --features holon-orgmode/di` → **309 tests run: 309 passed**, FSEXIT=0.
- `cargo nextest run --no-fail-fast -p holon-integration-tests --test org_suite nested_vault_copy_dup_slug` → **1 passed, 39 skipped**.
- The six tests that matter all PASS by name: `every_leg_agrees_across_symlinks`, `every_leg_agrees_on_every_dot_directory`, `every_leg_agrees_on_a_nested_gitignore`, `duplicate_block_slug_tests::a_second_file_claiming_a_blocks_slug_is_refused_whole_and_disclosed`, `…::the_refusal_lifts_once_the_claimant_drops_the_slug`, `poll_new_files_containment::refusal_lifts_when_the_claimant_leaves_disk`.
Log: `…/scratchpad/verify-dupslug/r34-gate.log`.

## (a) My two earlier divergences are CLOSED — independent probe, not the lane's tests
I built my own out-of-tree probe crate (`…/scratchpad/verify-dupslug/probe`, path-dep on `holon-filesystem`, own `CARGO_TARGET_DIR`) that compares `walk_directory` against `VaultFilter::admits` path-by-path over four fixtures, including two shapes the in-tree tests do NOT cover. **Zero divergence on all 15 comparisons** (`…/scratchpad/verify-dupslug/probe.log`):
- A (git repo): root `.gitignore` `vendor/`, nested `sub/.gitignore` `scratch.org` → both legs refuse `vendor/dep.org`, `sub/scratch.org` AND `sub/deeper/scratch.org` (nested rule inherited downward), admit `sub/Keep.org`. Symlinked dir `mirror → .claude/worktrees/x` refused by both (`beyond the symlink '…/mirror'`); symlink to a plain sibling file `Alias.org` ADMITTED by both. `.claude/…` refused as `hidden segment '.claude'`.
- C (my own, not in-tree): `.gitignore` in a PARENT dir with the repo root above the vault → both legs refuse `hiddenbyparent.org`.
- D (my own, not in-tree): symlink to a directory entirely OUTSIDE the vault → both legs refuse `link/Ext.org`.
The refusal reasons are typed and legible (`VaultRefusal::{Hidden,BeyondSymlink,Gitignored}` naming the exact `.gitignore` file). The in-tree parity tests also carry fixture-sanity asserts (`"fixture is wrong: …"`), so they cannot pass vacuously.

## (c) `require_git` — measured, fixture B
Outside a git repository the filter honours **NO gitignore at all** — it ignores *nothing*, matching `ignore`'s `require_git(true)` default that the boot walk uses. Measured: in `plainB` (no `.git`), with `.gitignore` naming `vendor/` and `secret.org`, **both** legs admit `A.org`, `secret.org` AND `vendor/dep.org`. `collect_gitignores` returns empty via `in_git_repo(root)`. This is the correct direction: honouring them there would drop files the boot walk ingests. It does not silently ignore everything.

## (b) Rev 4 block-slug refusal
- **Two files sharing a slug → second refused whole, first intact, disclosure names both + slug**: CONFIRMED by the passing test, whose asserts are the right ones — outcome `RefusedWhileClaimed(ClaimedId::BlockSlug(block:dupblk-shared))`, `dupblk-second-only` absent from the store while `dupblk-shared` remains, `First.org` bytes byte-equal to the original, and exactly ONE disclosure containing all three of `First.org` / `Second.org` / `dupblk-shared`. Placement is right: the check sits after the parse and **before document resolution/minting** (`file_sync_controller.rs:3111`), so a refused file leaves nothing behind.
- **Fixing the second file's claimant → ingested without deleting the first**: CONFIRMED by `the_refusal_lifts_once_the_claimant_drops_the_slug`. Note the test does NOT re-ingest `First` before retrying `Second`, so it genuinely exercises the release-by-RE-READ path (`live_block_claimant_of` re-reads and re-parses the recorded home rather than trusting `block_home`).
- **Deleting the FIRST file → the second becomes ingestible**: CONFIRMED at code level only. `live_block_claimant_of` returns `Ok(None)` on `ErrorKind::NotFound` (line 2000), and `on_file_deleted` drops that file's claims (`block_home.retain`, line 2227). `refusal_lifts_when_the_claimant_leaves_disk` is the DOCUMENT-`#+ID:` case (`#+ID: {SHARED_ID}` fixture) — there is no block-slug twin of it. **GAP** (see below).
- Retry loop verified: `poll_new_files` quarantines a refusal with its `ClaimedId` and lifts the skip via `claimant_still_holds` (1960), so a refused file is retried when its own bytes change OR when the claimant stops holding — not only on a touch of the refused file.
- IO/parse errors on the claimant are propagated with context and, in the poller, disclosed and treated as "claim still stands" (fail-safe, never silent).

## Defects (evidence only)
1. **Stale doc comment + a real repeating cost.** `poll_new_files` says the re-check "takes the stat above plus, for a refusal, one **stat** of the CLAIMANT". For `ClaimedId::BlockSlug` it is not a stat: `claimant_still_holds` → `live_block_claimant_of` performs a full `read_to_string` + `parse` of the claimant. With a quarantined duplicate present, that is one read + one org parse of the claimant on **every 2 s discovery tick, indefinitely**. Correct, but the comment misdescribes it and the cost is unbounded in time.
2. **`VaultFilter` gitignores are read once, at watcher construction**; the boot walk re-reads them on every 2 s discovery walk. A `.gitignore` added or edited after startup therefore makes the legs disagree again until restart — the exact drift class this lane exists to kill. The struct doc ("Built once per root … exactly as the walk reads it at the start of each walk") states the first half but not the consequence.
3. **Parent-`.gitignore` collection is unbounded upward**: `collect_gitignores` walks `root.ancestors().skip(1)` to `/`, while `ignore`'s `parents(true)` stops at the repository root. My fixture C could not separate the two (repo root was the immediate parent). A `.gitignore` above the repo root would be honoured by the filter and not by the walk. Unverified, narrow.
4. **Harness-leg fidelity is disk-dependent**: for a purely virtual root (`/holon-virtual/vault`), `root.exists()` is false → no gitignores collected, and `symlink_metadata` always fails → the in-memory scan degrades to hidden-segments-only. The parity tests avoid this by rooting the in-memory vault at a REAL temp dir. Inherent, but it means the harness leg is only as faithful as the disk beneath it.

## Gaps
- No test for "delete the claimant FILE → a block-slug-refused file becomes ingestible" (only the `#+ID:` analogue exists).
- I did not re-measure the lane's rev-3/rev-4 reds this round; I substituted the stronger independent 4-fixture divergence probe above plus the fixture-sanity asserts inside the parity tests.
- Earlier gaps that persist: no measured end-to-end red isolating the watcher leg; full `-p holon-integration-tests` suite still not run.

## Verdict: CONFIRMED
