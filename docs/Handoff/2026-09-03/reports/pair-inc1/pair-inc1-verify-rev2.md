# Verify rev 2 — lane `pair-inc1` — **REFUTED (narrow)**

Verifier workspace: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/pair-inc1`
(`pwd` printed at the head of every gate log). No jj/git writes. Every edit made
for teeth was restored byte-for-byte via cp-aside-and-copy-back, sha256 proven.

**Verdict.** The functional fix is REAL and independently reproduced. The claim
that **rev 2 "closed" two residuals is REFUTED**: neither close is pinned by a
test that fails when the behaviour is reverted. Close (4) is pinned only by a
truth-table assertion over the predicate function; its actual integration can be
neutered with the whole suite staying green. Close (5) has no covering test at
all.

---

## Check 1 — tree identity (PASS, with a brief discrepancy)

`jj -R WS status` lists **12** changed paths, not the 7 the task brief names.
The 5 extra are consistent with the lane report, so the brief was
under-specified, not the tree:

- `crates/holon-loro/src/loro_share_backend.rs` (report §8 residual 2)
- `crates/holon-loro/src/loro_sync_controller.rs` (report §3 — the main fix)
- `crates/holon-loro-wiring/src/loro_module.rs` (report §3b)
- `docs/Testing/bugfunnel/entries/2026-09-02-receiver-projection-stalls-after-one-failed-reconcile.md` (M)
- `docs/Testing/bugfunnel/entries/2026-09-02-receiver-sql-loses-a-block-its-loro-tree-still-holds.md` (A)

Markers on disk:
- `ProjectionPass` — `crates/holon-core/src/downstream_projection.rs:31,39,58,60`
- `sql_projection_lag` — `crates/holon-integration-tests/src/pbt/composed/two_instance.rs:229`,
  `crates/holon-integration-tests/tests/two_instance_composed_pbt.rs:701,702`

## Check 2 — gates re-run from scratch (PASS, with a flakiness finding)

| gate | log | result |
|---|---|---|
| nextest `holon-core -p holon-filesystem -p holon-app -p holon-architecture-tests` | `lane-logs/verify-r2-units-2026-09-03-0110.log:530` | **416 tests run: 416 passed, 1 skipped** |
| ↳ `loro_doc_escapes_match_the_allow_list` | same log `:336` | PASS |
| ↳ `archlint_all_passes` | same log `:527` | PASS |
| ↳ `downstream_flush_tests::*` (4) | same log `:431-434` | PASS |
| `lane-logs/arch.sh` (fmt --check, `just analyze-arch`, archlint) | `lane-logs/verify-r2-arch-0210.log` | exit **0**, 7/7 |
| nextest `-p holon-loro` (clean tree) | `lane-logs/verify-r2-loro-clean-0222.log:414` | **337 passed, 3 skipped** |
| `bugfunnel.py check` | — | 613 entries, 0 problems, exit 0 |

`two_instance_composed_pbt` binary, 3 clean runs:

| run | log:line | Summary | failing set |
|---|---|---|---|
| 1 | `lane-logs/verify-r2-pbt-run1-0115.log:4609` | 12 run: 9 passed, **3 failed**, 1 skipped | `one_way_share_converges_on_the_receiver`, `…_over_iroh`, `two_instance_composed_pbt` |
| 2 | `lane-logs/verify-r2-pbt-run2-0125.log:4602` | 12 run: 9 passed, **3 failed**, 1 skipped | same three |
| 3 | `lane-logs/verify-r2-pbt-run3-0131.log:249` | 12 run: 10 passed (1 **leaky**), **2 failed**, 1 skipped | `one_way_share_…`, `…_over_iroh` — the composed machine **passed** |

Every red carries the org-write-back signature
(`the receiver's store converged but its ORG files are missing … lost on
restart`, panics at `two_instance_composed_pbt.rs:270` and
`composed/harness.rs:1137`) — run 1 lines 205 / 231 / 309. Zero reds carry the
new `projection is BEHIND` oracle message. **PASS-WITH-NOTE** per the brief.

**FINDING (flakiness).** Report §6 states the class as "three tests fail".
Across my four full-binary runs the failing set varied — run 3 had the composed
machine green, and the teeth-B run (`verify-r2-teethB-pbt-0202.log:4492`) had
`one_way_share_converges_on_the_receiver` green while the machine was red. The
class is **non-deterministic**, so §6's per-test attribution is not reproducible
as written, and a 2-of-3 failing machine cannot serve as a stable weave baseline.

The lane's claim that the two pins are green is independently reproduced:
`cross_peer_indent_then_join_stalls_the_receiver_projection` PASS in every run
(e.g. run 1 `:257`, 19.9 s), `concurrent_two_writer_pair_converges` PASS
(run 1 `:4607`, 272 s).

**Gate-integrity note on the lane's own script.** `lane-logs/gates-r4.sh` uses
`set -uo pipefail` (no `-e`), runs `cargo fmt --all` (a tree WRITE) before its
own `--check`, and captures `cargo check … | tail -3; echo check_exit=$?` — that
`$?` is **tail's** exit status, not cargo's. Any `cargo check` failure recorded
through that line reads as `check_exit=0`. Known hazard class
(`gate-false-green-bad-cd`).

## Check 3 — TEETH (the refutation)

Baseline sha256 (scratchpad `teeth/baseline.sha256`):
`f5641170…e32d6  crates/holon-loro/src/loro_sync_controller.rs`
`b55097ca…ae27  crates/holon-integration-tests/tests/two_instance_composed_pbt.rs`
Both re-verified identical after every teeth step (final check in the same call
as the last restore).

### Close (4) — "withheld deletes are counted correctly"

**A1 — invert the predicate** (`withheld_deletes_are_owed(armed) -> bool` body
`armed` → `false`):
RED, exactly one test, as claimed —
`holon-loro loro_sync_controller::orphan_gate_tests::a_withheld_delete_is_owed_once_armed_but_never_during_the_unarmed_boot_window`
(`lane-logs/verify-r2-teethA1-0145.log:281`, `loro_exit=100`).

**A2 — keep the predicate, break the integration.** The line the close is
actually about is `loro_sync_controller.rs:1066-1068`:

```rust
if withheld_deletes_are_owed(armed) {
    ungrounded += withheld;
}
```

I changed the condition to `withheld_deletes_are_owed(armed) && false`, so
withheld deletes never reach `ungrounded` — i.e. exactly the pre-rev-2
behaviour the close claims to have fixed (`ProjectionPass` can never report a
withheld delete; `seeded` is not cleared; the share backend's prune-delete
guard cannot detect its own case).

- `-p holon-loro`: **337 tests run: 337 passed** — `lane-logs/verify-r2-teethA2-0150.log:406`,
  byte-identical to the clean baseline `verify-r2-loro-clean-0222.log:414`.
- `two_instance_composed_pbt` binary: **12 run: 10 passed, 2 failed, 1 skipped**
  (`lane-logs/verify-r2-teethA2-pbt-0155.log:214`) — the same two org-write-back
  reds as the clean run 3. No new red.

**DEFECT.** The rev-2 close is pinned only by a tautology: the test asserts the
predicate function returns its own literal. The behaviour the close exists to
guarantee — that a withheld delete becomes `ProjectionPass::Incomplete`, forces
`seeded=false`, and trips the share backend's prune-delete guard — is covered by
**no test in `holon-loro` (337) nor in the two-instance binary (12)**. This is a
`no-tests-of-tests` inversion: a future edit that removes the accumulation while
leaving the named predicate in place ships green.

### Close (5) — "the lag check happens before the early return"

I reverted the ordering in `assert_converged`
(`two_instance_composed_pbt.rs:715-740`): moved the `writes == 0` early return
back ABOVE the `projection_lag.is_empty()` assertion, restoring the exact
behaviour the close describes as the residual ("skipping it let such a draw hide
a lagging projection").

Full binary run: **12 run: 10 passed, 2 failed, 1 skipped**
(`lane-logs/verify-r2-teethB-pbt-0202.log:4492`). Both reds are the
org-write-back signature (`:179`, `:230`). **No new red, no test named the
ordering.**

**DEFECT.** Close (5) has **zero** covering tests. Nothing in the suite
distinguishes lag-before-return from lag-after-return, so the residual is closed
in the source only, not in the gate. (Structurally expected: the four
deterministic scripts all drive `writes > 0`, and the property would need a draw
whose every precondition is unsatisfiable AND a lagging projection in the same
case.)

## Check 4 — bugfunnel entry + re-ignored pin (PASS)

- Entry exists: `docs/Testing/bugfunnel/entries/2026-09-02-receiver-sql-loses-a-block-its-loro-tree-still-holds.md`
  (`gap: ORACLE`, `status: OPEN`). `bugfunnel.py check` → 613 entries, 0 problems.
- Reason string, `two_instance_composed_pbt.rs:942`:
  `#[ignore = "OPEN, different defect: the receiver's block_raw loses block:c2 while its Loro tree keeps it — see the 2026-09-02-receiver-sql-loses-a-block bugfunnel entry"]`
  and the doc comment at `:939-940` names the full filename. Names the entry: YES.
- **Independently reproduced the parked red** (`lane-logs/run-pins.sh`,
  `--run-ignored all`, log `lane-logs/verify-r2-pins-0215.log`):
  - `:183` `cross_peer_indent_then_join_stalls_the_receiver_projection` **PASS** 8.4 s
  - `:184` `owner_heavy_…` **FAIL**, `:217-219`:
    `owner-heavy indent+join: … a side's SQL projection is BEHIND its Loro tree after 5 write(s) … 1 divergence(s): receiver block:c2: held in Loro, ABSENT from block_raw (parent block:parent)`
  Byte-matching the recorded reason. §10 CONFIRMED.

## Check 5 — every `flush()` caller (PASS)

`rg 'flush\(' crates` — three real `DownstreamProjection::flush` callers; the
rest are `AsyncWrite`/socket/OTel flushes in unrelated crates.

| caller | handling |
|---|---|
| `crates/holon-loro/src/loro_share_backend.rs:1641` | `pass.withheld() > 0` → `return Err(...)` naming the count |
| `crates/holon-app/src/wiring.rs:586` | `match` with an explicit `Ok(pass) if pass.withheld() > 0` → `tracing::warn!` arm (documented best-effort seed flush) |
| `crates/holon-filesystem/src/file_sync_controller.rs:266` | inside `flush_downstream_with_redrive`, 3 attempts then `bail!` |

**No discarded `Result`, no `let _ =`, no ignored `Incomplete`.** CONFIRMED.

## Residuals

1. **(P1, gate) Close (4) has no behavioural teeth** — teeth A2 above.
2. **(P1, gate) Close (5) has no covering test at all** — teeth B above.
3. **(P2) The "pre-existing write-back" red class is flaky**, not the fixed set
   of three §6 names; the composed machine `two_instance_composed_pbt` failed
   2 of 3 clean runs and one run reported `1 leaky`. A weave gate built on §6's
   list will be unstable.
4. **(P2) `lane-logs/gates-r4.sh` cannot fail on `cargo check`** — `$?` after a
   pipe captures `tail`. Also missing `set -e` and it writes the tree
   (`cargo fmt --all`) before checking it.
5. **(P3) Report §11 item 3 contradicts §8b item 4** ("Withheld DELETES still do
   not mark a pass incomplete" vs "withheld DELETES now reach `ProjectionPass`").
   §11 is stale relative to rev 2.
6. **(P3) `ProjectionPass` is not `#[must_use]`.** All current callers handle it,
   but nothing structurally stops the next one from discarding it.
7. **(P3) The task brief's "7 modified files" understates the diff** — 12 paths,
   including the primary fix file `loro_sync_controller.rs`.
