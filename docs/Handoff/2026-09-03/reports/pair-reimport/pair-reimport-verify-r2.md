# Rev 2 security + crash-safety re-verification — `pair_with_owner` (D78.d)

**Verdict: CONFIRMED.** Rev 2's staging/marker/archive/promote design withstands
adversarial kill-point analysis, idempotence probing, pre-dial validation
checks, content-fidelity checks under archive corruption, the bare-`LoroDocument`
staging fix, and a secrets grep. All eight rev-1 findings (HIGH-1..4, MEDIUM-1,
MEDIUM-2) are addressed with evidence, not just narrative. One test anomaly is
recorded below as unresolved but not attributable to this design.

Workspace: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/pair-reimport`,
uncommitted lane snapshot on `wip/pair-reimport` (mzlvkunm), base `main`
4e2ee368. No jj/git write command was run. A temporary probe test file
(`crates/holon-loro/tests/verify_r2_probe.rs`) was added, run, then deleted;
`jj diff --stat | shasum -a 256` before and after the probe is identical
(`3da1523ec0…`), proving the working copy reverted byte-for-byte.

## 1. Kill-point matrix — CONFIRMED

`crates/holon-loro/src/pairing_swap.rs:161-203`
(`complete_interrupted_swap`). Probed all 8 combinations of
{live, staged, archived} × {present, absent} with a marker written (superset
of the 4 states the shipped unit tests cover), plus 4 extra adversarial cases.
Evidence: `lane-logs/verify-r2-probe-1.log`.

| live | staged | archived | Outcome |
|---|---|---|---|
| F | F | F | `Err` — "left neither a live … nor an archived one" |
| F | F | T | `Err` — "archived … but the owner's document is at neither" |
| F | T | F | `Err` — same as row 1 |
| F | T | T | `Ok(ReimportOwed)`, promote runs, live present after |
| T | F | F | `Ok(Settled)`, marker removed |
| T | F | T | `Ok(ReimportOwed)`, live untouched (already promoted) |
| T | T | F | `Ok(Settled)`, marker removed, staging untouched |
| T | T | T | `Ok(ReimportOwed)` |

Invariant checked programmatically for all 8 rows: every `Ok` leaves a live
global document on disk (`live_after == true`); every `Err` leaves none
(`live_after == false`, i.e. no code path both errors and silently produces a
usable-looking empty store). Holds in all 8 cases — no case boots empty
without an error.

Extra probes:
- **Stale `staging-<ts>` from an earlier attempt** coexisting with a current
  marker's staging/archive: ignored, doesn't interfere; recovery uses only the
  paths the marker names (`probe_stale_staging_from_an_earlier_attempt`).
- **Second attempt while a marker exists**: `pair_with_owner` never reads
  the marker (only boot recovery does), so a same-process retry mints a new
  stamp and a new marker, overwriting the file. `probe_second_attempt_overwrites_the_marker`
  shows the *first* attempt's archive becomes unreferenced on disk (garbage,
  not data loss — its content is still physically present, just not named by
  the current marker) while the second attempt's archive holds whatever was
  live at that moment. This matches the design note that `pair_with_owner`
  itself, not the recovery path, is guarded against re-entry by the
  `AlreadyPaired` refusal (see §2) — a same-process double-invocation before
  the first one's `pairing.json` is written is a real edge case but requires
  the caller to fire `pair_with_owner` twice concurrently without waiting for
  the first `Result`, which the single `device.pair_with_owner` operation
  entry point does not expose (no re-entrant call site found in
  `device_pairing_op.rs` or `frontends/gpui/src/share_ui.rs`). Recorded as a
  residual, not a reachable defect.
- **Corrupt marker JSON**: `read_marker` fails loud
  (`… is not a pairing marker: key must be a string …`), never silently
  treated as absent. Boot's `.expect(...)` at
  `crates/holon-loro-wiring/src/loro_module.rs:106` then aborts boot rather
  than adopting a guess — consistent with D94 (already routed to Martin, not
  counted here).

## 2. Idempotence / HIGH-2, HIGH-4 — CONFIRMED

`crates/holon-loro/src/device_pairing_op.rs:808-834` (`refuse_unpairable`):
mounts check, then `AlreadyPaired` (reads `pairing.json` via
`pairing_swap::read_record`), then per-ticket checks — all before
`stage_owner_documents` (the first step that touches the wire) and long
before any archive/promote/wipe. `pairing_a_device_that_is_already_paired_is_refused`
is in the green two-instance suite (§7).

**Residual, not a new defect**: `AlreadyPaired` is enforced by one file,
`<store>/pairing.json`. Deleting it by hand and retrying re-enables the
destructive path: `own_content` (`device_pairing_op.rs:567`) has no concept of
"already adopted from a prior pair" — it filters only the seeded app families
(layout, journals machinery), so a manually re-triggered pair would capture
the *first* owner's whole vault as "this device's own content" and write it
into the second owner's store, reproducing rev 1's HIGH-4 shape. This requires
deliberate filesystem tampering with the pairing record (an attacker who can
do that can already read/write the store directly), so it is not rated as a
live defect, but it is the single point of failure for HIGH-4 and worth a
one-line note in the design doc.

## 3. Validation before dial — CONFIRMED

`pair_with_owner` (`device_pairing_op.rs:1037-1043`): `decode` → `refuse_unpairable`
→ `stage_owner_documents` (the dial). No marker, archive, or wipe happens
before staging succeeds; a stubbed/unreachable owner therefore fails inside
`stage_owner_documents` before any store mutation, matching the design table.
Confirmed by code reading (no write call precedes staging) and by the green
`an_owner_that_cannot_be_reached_leaves_this_devices_store_intact` test in the
full suite run (§7).

## 4. Content fidelity — CONFIRMED

- **Archive read failure is loud and names the path.** `reimport_from_archive`
  (`device_pairing_op.rs:846-863`) calls `LoroDocument::load_from_file`, whose
  `.context("opening the pre-pair document {path} to re-import from")`
  wraps the error. Probed directly: an empty archive file yields
  `Err(Decode error: (Invalid import data))`; a garbage-bytes file yields
  `Err(Decode error: (Invalid magic bytes))` — both propagate through
  `anyhow::Context`, so the caller sees the archive path. No silent empty
  read.
- **Same-id conflict.** `plan_reimport` (`device_pairing_op.rs:303-346`):
  a captured id the adopted store already holds, with different content, is
  never overwritten — the phone's text is kept verbatim as a new block whose
  id is `<id>-before-pairing`, parented under the owner's live node of that
  id, carrying `pairing_conflict_of` = the original id
  (`conflict_copy`, `device_pairing_op.rs:262-273`). Confirmed by the unit
  test `a_block_the_owner_also_holds_with_different_content_is_kept_under_the_owners_node`
  (green, §7) and by code reading — `snap.block.clone()` copies content
  unmodified, only `id` and the property map change.

## 5. Bare-`LoroDocument` staging — CONFIRMED

`stage_owner_documents` (`device_pairing_op.rs:616-627`) builds the staged
document with `LoroDocument::new_with_peer_id(GLOBAL_DOC_ID, Some(rand::random()))`
— not through `self.store.get_doc`, so no `initialize_schema` replay under a
third peer. Verified empirically, not just by absence-of-call-site reasoning:
both full `two_instance_composed_pbt` reruns after this session's network
outage show **zero** `Missing in parent's children` warnings
(`lane-logs/verify-r2-full-1.log`, runs 1 and 2, 27/27 passed each,
`grep -c "Missing in parent's children"` = 0). The staged document's doc id is
`GLOBAL_DOC_ID` ("holon_tree"), structurally distinct from the layout scope,
so it cannot carry layout data by construction — the layout document lives
under a different `DocScope` and is never touched by `stage_owner_documents`.

## 6. Secrets / bearer data — CONFIRMED

`PairingMarker` and `PairingRecord` (`pairing_swap.rs:38-56`) carry only
`archive`/`staging` paths, the owner's endpoint id (a string derived from
`ticket.addr.id`, not a secret), and timestamps/container ids. Grepped every
site that constructs or writes these structs
(`device_pairing_op.rs:1049-1084`, `:894-903`) — `ticket.capability` is used
only at the dial call site (`device_pairing_op.rs:659`) and never stored. The
one place an invite string is emitted is `pair_offer`'s own operation response
(`device_pairing_op.rs:1008-1009`) — that is the intended API: the owner side
must hand the bearer invite to the new device out-of-band. It is not logged
and not written to any pairing file. The SAS ceremony gap (invite = bearer
data for its whole TTL) is unchanged from rev 1 and already flagged as a
residual, not counted as a new defect.

## 7. Gate rerun — CONFIRMED, with one unresolved anomaly

- Unit tests, `holon-loro` pairing/swap/device_pairing filter: **23/23
  passed** (`lane-logs/verify-r2-gate-2.log`), including all 5
  `pairing_swap::tests::*` kill-point tests and the 4 new
  `device_pairing_op::tests::*` conflict/idempotence/orphan tests.
- `two_instance_composed_pbt` first attempt: 26/27 passed, 1 failed —
  `pairing_a_receiver_that_was_used_standalone_keeps_its_content_and_one_node_per_fixed_id`
  FAILED at 17.42s under a 27-test parallel nextest run, on a machine shared
  with 7 other lanes. The failure's stderr was **not captured** — this
  session's own gate script piped that stage through `tail -25`/`tail -40`
  before logging, which is a verification-script defect, not evidence the
  code is broken.
- Following a mid-verification network outage, the file was rerun **twice in
  full**, uncapped: `two_instance_composed_pbt` **27/27 passed, both runs**
  (`lane-logs/verify-r2-full-1.log`, runs 1 and 2), zero Loro warnings. The
  specific keystone test was also rerun **3/3 in isolation**, all green
  (`lane-logs/verify-r2-keystone-1.log`).

**Classification: unresolved, not attributed to this lane's design.** The
single failure did not reproduce in 5 subsequent runs (2 full-file, 3
isolated) after the machine's contention eased. It cannot be confirmed as a
regression because the causing error text was never captured, but it also
cannot be waved away as a proven flake for the same reason. Recommend: if it
recurs, capture full `--no-capture` output before the next re-verification
rather than truncating with `tail`.

## Open design points routed to Martin — not counted as defects

- **D93** — the conflict-copy naming/UI convention (`pairing_conflict_of`,
  `<id>-before-pairing`) is invented in this lane; needs a ruling on how it
  should read in the UI.
- **D94** — `complete_interrupted_pairing` is called with `.expect(...)` at
  boot (`crates/holon-loro-wiring/src/loro_module.rs:304`); a permanently
  failing re-import blocks boot rather than booting degraded.

## Severity-tagged residuals (informational, no fix required by this pass)

- **LOW** — `AlreadyPaired` is a single on-disk file
  (`<store>/pairing.json`); its deletion re-opens the HIGH-4 shape from rev 1.
  See §2.
- **LOW** — a same-process double-invocation of `pair_with_owner` before the
  first call's marker/record land is not structurally prevented at the
  function level (only by there being no re-entrant call site today). See §1.
