---
id: 2026-07-30-quarantined-boot-app-permanently-shows-red
date: 2026-07-30
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  `Projects/Holon.org` is QUARANTINED on EVERY boot, so the app permanently
  shows the red banner `File sync degraded (bad org file) — OrgMode initial
  scan failed for 1 file` and write-back to that file is refused forever.
  Exact error, reproduced headlessly over a faithful 104-file copy of the live
  vault (scratch harness
  `crates/holon-integration-tests/tests/scratch_vault_scan_repro.rs`,
  `HOLON_SCAN_DIR=/Users/martin/Workspaces/pkm/holon-pkm`): `INGEST DATA LOSS:
  write-back of .../Projects/Holon.org would DELETE 23829 block(s) that exist
  on disk but did NOT survive ingest (source has 24084 block(s), projection
  has 3)` → `[FileSyncController] REFUSING write-back ... the file is
  quarantined until a clean re-ingest`. Shape: the file is 3.3 MB / 96,865
  lines / 24,084 `:ID:` blocks with FOUR top-level headlines — three empty `*
  Frontends` roots with three DIFFERENT `:ID:`s plus one `* Tasks` holding the
  other 24,080 — and its subtree is duplicated ~1004× (every one of a sample
  of headlines appears exactly 1004 times). Its page identity collides with
  the sibling DIRECTORY `Projects/Holon/`: in the live DB the doc root
  `block:cb7d94d4-75fa-4043-a6a7-a58e509b24e0` has 28 children, and they are
  the sub-PAGE roots from `Projects/Holon/*.org` (e.g. `ClaudeCode`), NOT the
  24,084 blocks the file claims — so the loss guard is CORRECT to refuse, and
  every one of the file’\s own blocks is orphaned from its document. Same file
  as the 2026-07-29 cold-boot O(N²) row, different failure: that one was
  throughput, this one is a permanent quarantine + degraded banner.
source_line: 1126
---

## Bug

`Projects/Holon.org` is QUARANTINED on EVERY boot, so the app permanently
shows the red banner `File sync degraded (bad org file) — OrgMode initial
scan failed for 1 file` and write-back to that file is refused forever.
Exact error, reproduced headlessly over a faithful 104-file copy of the live
vault (scratch harness
`crates/holon-integration-tests/tests/scratch_vault_scan_repro.rs`,
`HOLON_SCAN_DIR=/Users/martin/Workspaces/pkm/holon-pkm`): `INGEST DATA LOSS:
write-back of .../Projects/Holon.org would DELETE 23829 block(s) that exist
on disk but did NOT survive ingest (source has 24084 block(s), projection
has 3)` → `[FileSyncController] REFUSING write-back ... the file is
quarantined until a clean re-ingest`. Shape: the file is 3.3 MB / 96,865
lines / 24,084 `:ID:` blocks with FOUR top-level headlines — three empty `*
Frontends` roots with three DIFFERENT `:ID:`s plus one `* Tasks` holding the
other 24,080 — and its subtree is duplicated ~1004× (every one of a sample
of headlines appears exactly 1004 times). Its page identity collides with
the sibling DIRECTORY `Projects/Holon/`: in the live DB the doc root
`block:cb7d94d4-75fa-4043-a6a7-a58e509b24e0` has 28 children, and they are
the sub-PAGE roots from `Projects/Holon/*.org` (e.g. `ClaudeCode`), NOT the
24,084 blocks the file claims — so the loss guard is CORRECT to refuse, and
every one of the file’\s own blocks is orphaned from its document. Same file
as the 2026-07-29 cold-boot O(N²) row, different failure: that one was
throughput, this one is a permanent quarantine + degraded banner.

## Root cause

the reported "org scan STALL" — an acceptance boot that sat at 23,891/25,163
blocks with 40 min of zero scan progress and was killed at 47 min — is NOT a
hang, and none of the three suspected confounds explains it. It is the
ingest of ONE FILE: `Projects/Holon.org`, 3.2 MB, 23,841 headlines, which is
94.7 % of the vault's blocks in a single file (the other 1,000 org files
hold the rest). The 23,891-block plateau is exactly that file's 23,841
headlines plus the ~50 from the ten small files ingested before it, so the
run was never "95 % through the vault" — it was 11 files in, inside file 11,
with the remaining 90 files untouched. Three suspects were named and all
three are exonerated by controls. (a) MCP CONTENTION (the claude-history
provider going live after its 600 s gate escape and running 62
`claude-history://projects` resyncs concurrent with the scan): refuted twice
over — first by ORDERING, the scan's last output is `18:39:06` and the gate
escape is `18:41:39`, so the silence begins 2.5 min BEFORE the provider is
live and the whole 18:32–18:39 silent window is pre-contention; then
decisively by a fresh CONTROL RUN (`/private/tmp/holon-stall-run1.log`),
release build, `integrations/` EMPTY so `[McpIntegrationsModule] Loaded 0
integration configs`, quiet machine (`pgrep -x cargo -x rustc` == 0 at
start), which reproduces the identical plateau — 11 files, 40 min cap, never
leaves `Projects/Holon.org`. Zero resyncs, same stall. (b) DEBUG-VS-RELEASE
build: refuted by that same release control, and independently by the
release baseline `/private/tmp/holon-cold-PINNED-2026-07-28T1940.log`, which
spends `16:07:20.158` → `16:27:47.465` = **20 min 27 s** on
`Projects/Holon.org` alone — 84 % of its entire 24.5-min boot — with
`resync_by_uri` count 0. (c) MACHINE BUILD LOAD: refuted by the
quiet-machine control. ROOT CAUSE, from two independent `sample` captures of
the control run (`/private/tmp/holon-stall-run1.sample.txt`,
`.sample2.txt`), stable across both and accounting for 83 % of process wall
time on the `TursoBackend::run_actor` thread: `process_actor_command` →
`Rows::next` → `Statement::step` → `Program::step` → `op_next` →
**`turso_core::incremental::cursor::MaterializedViewCursor::next` →
`do_seek` → `read_btree_delta_entry` → `dbsp::HashableRow::new` →
`compute_hash` → `Hash128::hash_values`** (sample 2 weights: next 2908,
do_seek 2225, read_btree_delta_entry 1543, HashableRow::new 1501,
compute_hash 1182). The cost is TURSO-SIDE and in our own fork: a
per-`next()` re-seek that re-reads and re-hashes btree delta entries,
allocating a fresh `Vec<Value>` per row (`raw_vec::finish_grow` churn is
visible under `hash_values`), i.e. an O(N²) matview cursor walk over the
delta set. Holon drives it thousands of times per file because the
post-write phase of `on_file_changed` does one matview-backed read per
distinct parent — `ordering.children(parent)` in the disk-order replay loop
plus the unconditional `get_blocks(document_uri)` doc walk
(`file_sync_controller.rs:2776` and the place loop below it) — and a
23,841-block file has thousands of distinct parents. Both phases are SILENT:
no logging inside a single file's ingest, which is the whole reason 40 min
of ordinary grinding was indistinguishable from a hang. Compounding it,
`finish_initial_scan`'s 30 s no-progress watchdog
(`file_sync_controller.rs:456`) is DOWNSTREAM of the per-file loop, so a
file that takes 47 min trips no watchdog at all — the acceptance run emitted
no diagnostic in 39 min, while the baseline, which did finish all 105 files,
reached the watchdog and failed loud with `block feed did not converge — no
progress for 30000ms with 172 of 25163 expected id(s) still missing`. NB
neither run ever printed `[OrgMode] initial scan complete`: the "24.5 min
degraded completion" baseline is a CONVERGENCE FAILURE, not a success.
ENVIRONMENT primary per the rubric's real-vault-scale clause: nothing about
the interaction is exotic — it is a cold boot — but the regime needs a
SINGLE file of ~24k blocks, and the keystone's generator emits
`[a-z_]+_[0-9]+\.org` files of a handful of blocks each, so no run has ever
been within three orders of magnitude ON ONE FILE. The per-file axis is the
missing one, distinct from total-vault-size: the 2026-07-28 ENV row two
below diagnosed the same 24.5-min boot as Loro per-block projection cadence,
measured with `loro: true`; this control ran `loro: false` (SqlOnly,
whole-op-vector-in-one-transaction) and stalls just as hard on a DIFFERENT
hot path, so SqlOnly is not the escape hatch that row's fix candidates
implicitly assume — both modes need the per-file fix. ORACLE secondary, and
worse than "no budget invariant exists": the release build produced NO stage
attribution AT ALL even though the control was launched with the documented
release opt-in `HOLON_LATENCY_SLO=1` (`logging.rs:64` `latency_slo_enabled`)
— zero lines matching `oracle|latency` in 352 log lines, where the debug
acceptance run had emitted `'boot_parse' took 7910ms` and `'boot_write' took
439388ms`. So a release dogfood boot, the configuration Martin actually
runs, has no latency signal whatsoever. Outcome verified; mechanism
ROOT-CAUSED AND FIXED 2026-07-29 in its own lane, and it is NOT the
`max_level_hint` clamp that was suspected here (the fmt `EnvFilter`s are
PER-LAYER filters, so they never veto the oracle layer; and
`Vec<Layer>::max_level_hint` returns `None` — unbounded — as soon as one
layer, `LatencySloLayer`, gives no hint). The real wall is COMPILE-TIME:
every `holon_latency` event was `tracing::debug!`, and the turso fork's
`workspace-hack` (`80ed4a4/workspace-hack/Cargo.toml`) enables
`tracing/release_max_level_info`, which feature-unifies across the whole
graph and deletes every `debug!` callsite from release binaries — so with
`HOLON_LATENCY_SLO=1` the opt-in branch ran, the layer was installed, and
there was simply nothing left to dispatch. FIX: all 17 emission sites
promoted `debug!`→`info!`, with default log volume held by an EnvFilter
directive instead of by the callsite level (`holon_latency=warn` by default
so the target's fail-loud `stage=e2e_expired` disclosures survive, `=info`
under the opt-in, an explicit `RUST_LOG` directive always wins) —
`crates/holon-frontend/src/logging.rs`. Red-first source-level guard
`latency_events_are_emitted_above_the_release_level_ceiling`
(`crates/holon-architecture-tests/tests/architecture_rules.rs`) fails on any
sub-INFO `target: "holon_latency"` callsite; it was RED on all 17. NOT FIXED
here — this lane was isolation-only; the fix is Turso-side
(`MaterializedViewCursor::do_seek` re-hash) plus a Holon-side reduction of
per-parent matview reads during scan, and both need a red-first keystone
with a single-large-file scale knob, which does not exist yet.)

## Missing piece

Split-doc-root / page-file-vs-directory identity family (siblings:
2026-07-21 duplicate-folder-page, 2026-07-28 `UnnamedPlaceholder`,
2026-07-29 split-doc-root, 2026-07-30 `:Page:`-tagged-child). The loss guard
itself RUNS in the keystone wiring; what is ungeneratable is the state — no
transition produces a document whose on-disk blocks have ALL been adopted by
other documents, and none produces a `Foo.org` + `Foo/` page-identity pair.
ENVIRONMENT secondary: the ~1004× duplication is a real-vault
write-back-amplification artifact with no analogue at test scale.

## Remedy

OPEN 2026-07-30 — diagnosed read-only, NOT fixed. Repro is deterministic
(1/1). Keystone repro attempted per the CLAUDE.md rule and structurally
impossible for the same reason as the sibling rows. Fix belongs to the
quarantine/`merge_blocks` family: `Projects/Holon.org` needs a
`merge_blocks`-style doc-root reconciliation against the `Projects/Holon/`
directory page (plus de-duplication of the 1004× explosion) before a clean
re-ingest can lift the quarantine.
