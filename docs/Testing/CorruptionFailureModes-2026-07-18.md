# Substrate-Corruption Failure Modes (2026-07-18)

Observed-vs-desired ledger for the `CorruptTurso` / `CorruptLoro` fault
transitions, capturing what the app does TODAY when its derived SQL layer
(Turso) or its persisted CRDT store (Loro) is damaged — **before** the
BootLadder recovery increments exist. These are the PBT guards that will later
flip from documented-red to green as those increments land.

- Transition family + oracle: `crates/holon-integration-tests/src/faults.rs`
- Red-guard rungs: `crates/holon-integration-tests/tests/substrate_corruption_faults.rs`
- Desired-behavior source: `docs/Plans/BootLadder-2026-07-18.md` (increments 2+)

## Why beside the keystone, not in `E2ETransition`

The one composed keystone (`general_e2e_composed_pbt`) boots **pre-started** and
has **no true storage reboot** — `SimulateRestart` only touch-writes org files
to re-ingest; nothing drops+reopens the Turso handle or rehydrates Loro from
disk (the real reboot is the unbuilt "F9" fork). So the `PreRestart` timing is
structurally unreachable in the keystone, and every corruption shape breaks the
keystone's steady invariants by construction — it can never be a green member of
the sequential alphabet. The shapes are therefore exercised through
`TestEnvironment` (real on-disk `test.db` + real-disk `holon_tree.loro`, with
`stop_app`/`start_app` reboot), exactly the `matview_reboot_duplicate_repro`
precedent ("kept: no reboot transition in keystone").

## The oracle — "no silent wrongness"

The invariant is NOT "everything works". After a corruption + a drive of the
UI-facing read path, the outcome is classified on the **fail-loud ladder**:

| Outcome | Meaning | Acceptable today? |
|---|---|---|
| `Survived` | read returns ≥ pre-corruption canonical rows, no error | yes (fault absorbed / org-recovered) |
| `TypedError` | boot/read returns a typed `Err` — no faked data | yes (fail-loud, priority 3) |
| `ObservedProblem` | ERROR log / background panic captured & disclosed (`otel-testing`) | yes (disclosed) |
| `Panic` | raw panic unwound on the driver thread | tolerated only until BootLadder Inc 1 (typed `BootError`) |
| `FaultRejected` | the substrate REFUSED the injection — nothing was corrupted | the run is vacuous; it is disclosed, never counted as recovery evidence |
| **`SilentDataLoss`** | read SUCCEEDS, presents fewer/zero canonical rows, **no error, no disclosure** | **NEVER — the one forbidden floor** |

Every guard asserts `outcome != SilentDataLoss`. When a BootLadder increment
lands, tighten the guard to assert the specific DESIRED outcome and un-ignore.

### Precedence — the data check runs FIRST

`classify` evaluates the row-count delta before any other rung, because the
forbidden floor is *defined* by that delta. Ordering it after the disclosure
check (as the original did) made `SilentDataLoss` structurally unreachable for
every `PreRestart` shape, since shutdown noise short-circuited to
`ObservedProblem` first.

1. `post_raw < pre_raw` → `SilentDataLoss` if nothing **attributable** was
   disclosed, else `ObservedProblem`.
2. boot/read returned `Err` → `TypedError`.
3. attributable disclosure with no data delta → `ObservedProblem`.
4. otherwise → `Survived`.

"Attributable" means the captured problem's **message** names the damaged
artifact (`block_raw`, `test.db`, `holon_tree`/`snapshot`,
`__turso_internal_dbsp_state_v1`) **and** is not on the
`AMBIENT_SHUTDOWN_NOISE` denylist. Two rules, both load-bearing:

- **Match the message, never the rendered problem.** `CapturedProblem`'s
  `Display` is `[{kind}] {target} ({loc}): {message}` — it interpolates the
  emitting module. Matching the rendered form makes every `holon_loro::*` site
  answer to the token "loro" regardless of what it actually said.
- **Use artifact-level tokens, not subsystem-level ones.** Message-only matching
  is necessary but *not* sufficient: prod prefixes many messages with the
  component name (`[LoroSyncController] …`, `[LoroBlocksDataSource] …`), so a
  bare "loro" still matches the body. The token must name the thing that was
  damaged.

Why this matters concretely: the boot-gate watchdog
(`loro_sync_controller.rs`, "boot gate never opened — org initial scan may be
wedged") fires *precisely when the vault is empty* — it is positively correlated
with the org-derived loss the guard exists to catch. Under either loose rule it
would classify a 23→0 row collapse as `ObservedProblem` and mask the floor.
`faults.rs`'s unit tests pin all of this with the real message strings from
`loro_sync_controller.rs`, `event_ring.rs`, and `loro_blocks_datasource.rs`.

### Phase boundary — a setup panic is never an outcome

`run_*_scenario` returns `Result<ScenarioReport, SetupFailure>` and sets an
`injected` flag the instant damage lands. A panic before that point is
re-raised and fails the test; only a post-injection panic may claim the `Panic`
rung. Without this, a panic in setup was silently laundered into an accepted
rung — which is exactly how the first port of this file shipped 7/7 guards
passing while injecting no corruption at all.

## Failure-modes table

Timing key: **MidRun** = damage while running, then read. **PreRestart** =
boot clean → `stop_app` → damage persisted artifact → `start_app` → read.

`Observed` is filled from `--ignored` runs of the per-shape guards on
`main` @ `026dacc2` (2026-08-02), reproduced identically across consecutive
runs. Every guard is GREEN: no shape reaches the forbidden floor.

| # | Substrate | Shape | Timing | Real failure class | Observed (today) | Desired (BootLadder) | Gap | Teeth |
|---|---|---|---|---|---|---|---|---|
| 1 | Turso | `DropBlockRawTable` | MidRun | matview reconcile dropping system tables | `TypedError` — DROP lands (FK off), post-read → `Err(no such table: block_raw)` | read fails loud OR degraded-in-place banner (Inc 5); never silent-empty | no degraded-in-place banner yet | **real** |
| 2 | Turso | `DropDbspStateTable` | MidRun | Android stale-epoch DBSP-state orphan | `FaultRejected` — Turso refuses: `Cannot drop system table __turso_internal_dbsp_state_v1_*` | typed error / rebuild IVM state; never stale-silent | **shape unreachable via SQL** | **none — vacuous, disclosed** |
| 3 | Turso | `TruncateDbFile` | PreRestart | corrupt/partial `test.db` on disk | `TypedError` — `start_app` → `Err(Failed to open Turso database …: short read on page 1: expected 512 bytes, got 2)` | no-Turso rung 1a: stay operational, lazy reseed (Inc 8); interim typed `BootError` (Inc 1) | Inc 1 already effectively met; rung 1a not built | **real** |
| 4 | Loro | `CorruptSnapshotBytes` | PreRestart | invalid-magic `holon_tree.loro` | `Survived` — a real ~11 KB snapshot is destroyed, reboot `Ok`, counts recover from org, **nothing attributable disclosed** | recover from org (rung 1b) OR recovery shell (rung 0); never silent-empty | see "the Loro blind spot" | **weak — cannot reach the floor** |
| 5 | Loro | `TruncateSnapshot` | PreRestart | truncated snapshot | `Survived` — as #4 | as #4 | as #4 | **weak** |
| 6 | Loro | `DeleteSnapshot` | PreRestart | missing snapshot | `Survived` — as #4 | as #4 (fresh doc from org, no data loss) | as #4 | **weak** |
| 7 | Loro | `CorruptSnapshotBytes` | MidRun | on-disk snapshot damaged under a live doc | `Survived` — in-memory doc unaffected, counts unchanged | latent until reload — a later reload MUST disclose, not silently drop | the reload leg is #4 | **weak** |

### The Loro blind spot (COVERAGE gap — guards #4–#7 cannot currently fail)

An earlier revision of this table claimed `ObservedProblem` and "Loro errors
disclosed" for #4–#6. **That was false**, and it was false for two compounding
reasons that are worth recording because both are easy to re-introduce:

1. **The oracle's observable is org-derived.** It counts `block_raw` / `block`,
   which every boot rebuilds from the org files. Loro damage therefore cannot
   move the number: the counts recover no matter what Loro did. A Loro-specific
   silent loss is invisible to this oracle.
2. **Prod discloses at WARN, the collector captures ERROR.**
   `crates/holon-loro/src/loro_document_store.rs` handles a bad snapshot with
   `tracing::warn!("Corrupted snapshot at {}: {}. Recreating.")`, deletes the
   file, and builds a fresh doc; `test_tracing.rs` records ERROR events only.
   So the one true disclosure is structurally unobservable here.

What made the false claim *look* true was ambient noise: every `stop_app`
emits a burst of `Actor channel closed` errors, and some carry the module path
`holon_loro::loro_sync_controller` — which contains "loro". Any substring
attribution laundered shutdown noise into "the corruption was disclosed", so
all 12 PreRestart combos could earn `ObservedProblem` **with no corruption
injected at all**. `AMBIENT_SHUTDOWN_NOISE` in `faults.rs` now denies those
messages before the signature match runs.

Consequence, stated plainly: **guards #4–#7 are green but toothless.** They
prove the app does not lose org-derived rows; they cannot prove anything about
Loro. Closing this needs a Loro-side observable (count the blocks in the
rehydrated Loro doc, not in `block_raw`) and/or promoting the snapshot-corruption
disclosure from WARN to ERROR. Until then they are regression ballast, not
coverage.

**#2 is vacuous** for a different reason: Turso hard-refuses `DROP` on its own
system tables, so the injection never lands. The Android stale-epoch class it
was written for is real; reaching it needs a file-level injector (damage the
DBSP state between `stop_app`/`start_app`) or a stale-epoch reopen.

_Non-meaningful combos are omitted: SQL-handle shapes have no `PreRestart` form
(they need a live handle), and `TruncateDbFile` is a no-op `MidRun` (latent file
shape). The ledger sweep still records them for completeness._

## BootLadder desired-behavior quotes (increments 2+)

From `docs/Plans/BootLadder-2026-07-18.md` (fallback ladder
3 Full → 2 Degraded-in-place → 1a No-Turso → 1b Loro-down → 0 Recovery shell):

- **After delete-Turso-DB** — the app STAYS OPERATIONAL; reseed is LAZY /
  INCREMENTAL (Increment 8, design-first).
- **Loro-store failure** — recovery shell for now, PLUS follow-up rung **1b-org**:
  continue read-WRITE from the org files while Loro is down; a later repaired
  Loro picks up interim edits as external file-sync edits.
- **Increment 1** converts boot-spine panics to a typed `BootError` (so shape #3
  should move `Panic → TypedError`).
