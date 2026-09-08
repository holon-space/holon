---
id: 2026-09-09-hidden-dir-vault-copy-invisible-at-boot-ingested-by-the-watcher
date: 2026-09-09
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  The boot vault walk and the live file watcher disagree on which paths are
  ingestable: `walk_directory` skips EVERY dot-directory (`ignore`'s
  `hidden(true)`), while the watcher's own `is_ignored` skips only `.git` and
  `.jj`, so an org file under `.claude/`, `.obsidian/` or `.logseq/` is
  invisible at boot yet ingested the moment it changes — and the live vault
  holds 12 stale nested copies of itself under `.claude/worktrees/agent-*/`,
  each carrying `Projects/Holon/Now.org` with the SAME `#+ID:` and headline
  `:ID:` slugs as the live file.
---

## Bug

Found by the `now-query` lane while driving `holon-mcp` against a copy of the
real vault (report `lane-report-now-query.md` §"Run 2", item 3 of "Vault
defects recorded, not fixed"). Write-back produced a `Now.org` of 265 lines
against a live file of 187, with `DONE` blocks resurrected that the live file
does not contain. Nothing reached the live vault — the lane worked on a copy —
but Martin's own instance watches the real vault, which is in exactly this
state.

Measured on the live vault (read-only), 2026-09-08:

- `.claude/worktrees/agent-*/` holds 12 full vault copies; 12 of them carry
  `Projects/Holon/Now.org`.
- The live `Now.org` and the `agent-a272b2c776c5386e2` copy declare the
  IDENTICAL `#+ID: 68809135-c2da-4e9b-ad02-d78336895688` and share 7 headline
  `:ID:` slugs; the copy is 111 lines against the live 187.
- `rg --files -g 'Now.org'` (the `ignore` crate's default walk, byte-identical
  in configuration to `walk_directory`) yields ONE path; `rg --files --hidden`
  yields thirteen. So the boot scan is clean and the exposure is the watcher.

## Root cause

Two filters, one question, no shared answer:

- `crates/holon-filesystem/src/fs_port.rs:222` `walk_directory` builds
  `ignore::WalkBuilder::new(root).hidden(true).git_ignore(true).git_global(true)`.
  `hidden(true)` drops every dot-prefixed component, so the boot scan never
  sees `.claude/worktrees/x/Now.org`.
- `crates/holon-orgmode/src/file_watcher.rs:53` `is_ignored` walks the path's
  components and returns true only for the literal segments `.git` and `.jj`,
  then defers to `.gitignore`. `.claude`, `.obsidian`, `.logseq` and every
  other dot-directory pass. `is_vault_relevant` (line 86) is the sole gate on
  the live-event path (`VaultFileWatcher::new`, line 173).

An agent jj-workspace under `.claude/worktrees/` is written constantly, so
every such write delivers a live `Modify` event that the watcher forwards and
`FileSyncController::ingest_file` acts on. The document-level guard that
exists — the duplicate-`#+ID:` refusal at
`crates/holon-filesystem/src/file_sync_controller.rs:2819` via
`live_claimant_of` (line 1885) and `disclose_duplicate_doc_id` (line 1981) —
keys on `self.doc_home`, an in-session map whose claimant is "whichever file
this session ingested FIRST". It therefore protects the live file only while
the live file happens to have been ingested first, and its own doc comment
says the scan order is arbitrary.

The ad-hoc `s == ".git" || s == ".jj"` string check is the defect's shape: the
walk's rule lived inside a third-party builder and the watcher's rule was
retyped by hand, so the two drifted with nothing asserting they agree.

## Missing piece

No test compares the boot walk's verdict with the watcher's verdict on the
same corpus — each filter had its own tests (`test_file_watcher_ignores_git_dir`,
`fs_port` walk tests) asserting its own behaviour, and neither could see the
divergence. Secondary COVERAGE: no keystone transition materialises a nested
vault copy under a hidden directory, and the keystone drives the in-memory
adapter rather than `VaultFileWatcher`, so the failing filter never runs in
its wiring.

## Remedy

FIXED. `crates/holon-filesystem/src/vault_path.rs` now owns
`hidden_vault_segment(root, path)` — the ONE rule naming the dot-segment that
excludes a path — and both the walk and the watcher consult it.
`crates/holon-orgmode/src/file_watcher.rs` `is_ignored` delegates to it
instead of matching `.git`/`.jj` by hand, and
`crates/holon-filesystem/src/in_memory.rs` `scan_directory` — which filtered
NOTHING, and is the reason the whole test fleet was blind to this — applies
the same rule (`scan_skips_hidden_entries_like_the_real_walk`). Covered by the
parity test
`walk_and_watcher_agree_on_every_dot_directory` and by
`a_hidden_nested_vault_copy_is_never_ingested`
(`crates/holon-orgmode/src/file_watcher.rs`), plus
`crates/holon-integration-tests/tests/org_suite/nested_vault_copy_dup_slug.rs`,
which boots a vault holding a live `Live.org` and a stale copy under
`.claude/worktrees/agent-boot/`, touches a second copy while the app runs, and
asserts the copy's own block never reaches `block_raw` and never appears in
the live file's bytes. Byte-identity is deliberately NOT the oracle: it would
false-red on the documented one-time renderer normalization (entry
`2026-08-05-entire-pre-existing-real-vault-write`).

The keystone can host this cheaply — a transition beside
`src/pbt/transitions/write_org_file.rs:44` plus one line in
`declare_e2e_transitions!` (`src/pbt/transitions/mod.rs:248`), with the live
`inv-blocks-match-ref/org` correspondence
(`src/pbt/composed/correspondences.rs:214`) supplying the oracle unchanged. It
was NOT added here: a new transition shifts the generation distribution for
every other keystone property, and the machine could not give the keystone
runs that change needs. The dedicated test carries the same oracle, so
promotion is a move.

Not closed by this entry: the block-level half. The refusal is document-level
only (`DUPLICATE_ID_SITE = "duplicate-doc-id"`,
`file_sync_controller.rs:287`), so two files with DIFFERENT `#+ID:` but
overlapping headline `:ID:` slugs still merge block-by-block with no claimant
check and no disclosure. Tracked as
`2026-09-09-block-slug-claimed-by-another-file-merges-with-no-refusal`.
