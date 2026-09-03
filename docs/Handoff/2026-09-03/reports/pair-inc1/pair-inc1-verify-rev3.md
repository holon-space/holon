# Verify rev 3 — lane `pair-inc1` — **CONFIRMED**

Workspace: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/pair-inc1`
(`pwd` printed at the head of every gate log). No jj/git write commands. Every
teeth edit restored by `cp` from a scratchpad backup and proven byte-identical
with `shasum -a 256 -c`.

Baseline (`scratchpad/teeth/baseline.sha256`), re-verified OK after every step:
- `crates/holon-loro/src/loro_sync_controller.rs` `f5641170…e32d6`
- `crates/holon-integration-tests/tests/two_instance_composed_pbt.rs` `74146cef…e016`

Note: the controller hash is **identical to rev 2** — rev 3 changed no
production code, only tests plus `lane-logs/gates-r4.sh`.

## (1) Tree identity — PASS
`jj diff --stat` = 18 paths, 1568(+)/315(-). `ProjectionPass` present at
`crates/holon-core/src/downstream_projection.rs:31,39,58,60`.

## (2) Lane rev-3 gates, re-run by me — PASS

| gate | log | result |
|---|---|---|
| `lane-logs/r3-check.sh` (fmt --check + `cargo check --workspace --all-targets` with the pbt features) | `lane-logs/verify-r3-check-a.log` | exit **0**, `CHECK_OK` present |
| `lane-logs/r3-loro-suite.sh` | `lane-logs/verify-r3-lorosuite-a.log:188` | **13 tests run: 13 passed, 1 skipped** |
| `lane-logs/r3-units.sh` (`holon-loro`, `holon-core`, `holon-filesystem`, `holon-app`, `holon-architecture-tests`) | `lane-logs/verify-r3-units-a.log:867` | **754 tests run: 754 passed, 4 skipped** |

`holon-architecture-tests` is inside `r3-units.sh`, so it is covered by that
754/754 line — no separate run needed.

`two_instance_composed_pbt` binary, 3 clean runs:

| run | log:line | Summary | failing set |
|---|---|---|---|
| 1 | `verify-r3-pbt-run1.log:4501` | 13 run: 10 passed, **3 failed**, 1 skipped | `one_way_share_converges_on_the_receiver`, `…_over_iroh`, `two_instance_composed_pbt` |
| 2 | `verify-r3-pbt-run2.log:4514` | 13 run: 10 passed, **3 failed**, 1 skipped | same three |
| 3 | `verify-r3-pbt-run3.log:245` | 13 run: 11 passed (1 **leaky**), **2 failed**, 1 skipped | the two `one_way_share_…` |

Every red carries the org-write-back signature (`the receiver's … ORG files are
missing … lost on restart`, `two_instance_composed_pbt.rs:270` /
`composed/harness.rs:1137` — run 1 lines 169/195/226). `grep -c "projection is
BEHIND"` = **0** in runs 2 and 3. **PASS-WITH-NOTE** per the brief.

The three pins are green in every run: `a_draw_that_exercised_nothing_still_judges_the_projection_lag`
(run 1 `:148`), `cross_peer_indent_then_join_stalls_the_receiver_projection`
(run 1 `:203`), `concurrent_two_writer_pair_converges` (run 1 `:4499`, 173 s).

## (3) TEETH — close (4) — **BITES**

Exact rev-2 inversion applied at `crates/holon-loro/src/loro_sync_controller.rs:1066`:
`if withheld_deletes_are_owed(armed) && false {` — withheld deletes never reach
`ungrounded`, the pre-rev-2 behaviour.

RED, and red for the right reason
(`lane-logs/verify-r3-teeth4-lorosuite.log:148,182`, exit 100):

`holon-integration-tests::loro_suite loro_projection_withheld_delete::an_armed_unsettled_pass_owes_the_delete_it_withheld`

```
panicked at .../loro_suite/loro_projection_withheld_delete.rs:77:5:
assertion `left == right` failed: the withheld delete is owed to the sink, so the pass did not converge
  left: Converged
 right: Incomplete { withheld: 1 }
```

This is exactly the assertion the rev-2 verdict found missing: the failure is on
`DownstreamProjection::flush`'s observable outcome, not on the predicate
function. Rev-2 residual 1 is CLOSED.

## (4) TEETH — close (5) — **BITES** (the reorder was KEPT, not deleted)

The report's claim matches the code: `assert_converged`
(`two_instance_composed_pbt.rs:715-741`) asserts `projection_lag.is_empty()`
FIRST, then returns on `writes == 0`.

I reverted the ordering (moved the `writes == 0` early return above the lag
assertion). RED (`lane-logs/verify-r3-teeth5.log:154,163,173`, exit 100):

`a_draw_that_exercised_nothing_still_judges_the_projection_lag` —
`test did not panic as expected at two_instance_composed_pbt.rs:771:4`
(`#[should_panic(expected = "SQL projection is BEHIND")]`).

Rev-2 residual 2 is CLOSED.

## (5) `gates-r4.sh` cargo-check exit — PASS
`lane-logs/gates-r4.sh` now redirects `cargo check … > "$CHECK_LOG" 2>&1`, takes
`check_exit=$?` on the very next line (cargo's own status), then `tail -3` the
file. The `cargo fmt --all` tree WRITE is gone — only `cargo fmt --all -- --check`
remains. Rev-2 residual 4 is CLOSED. (`set -uo pipefail` without `-e` stays, but
each step's exit is now echoed individually, which is the script's design.)

## (6) §11.3 vs §8b.4 — RECONCILED
§11 item 3 now reads "An **UNARMED** pass's withheld deletes still do not mark
it incomplete (§8b item 4 has the armed half)", which is consistent with §8b
item 4's armed-only scope. Rev-2 residual 5 is CLOSED.

## Residuals still open (none refute rev 3)

1. **(P2) The write-back red class is still flaky** — rev-2 residual 3 stands:
   3 reds / 3 reds / 2 reds + 1 leaky across my three runs. A weave gate that
   pins a fixed failing set will be unstable; gate on the signature, not the
   name list.
2. **(P3) A new `unused_variables` warning** the lane introduced:
   `crates/holon-integration-tests/tests/two_instance_composed_pbt.rs:128`
   `let (caps, handle, _) = boot_two_instances(...)` — `handle` unused
   (`verify-r3-check-a.log:1095`). Cosmetic; `cargo check` is not `-D warnings`.
3. **(P3) `ProjectionPass` is still not `#[must_use]`** (rev-2 residual 6).
4. **(P3) The owner-heavy parked pin** (§11 item 1) is unchanged — not re-run
   here; rev 2 already reproduced its recorded reason.
