---
id: 2026-11-21-two-peer-fixpoint-was-six-rounds
date: 2026-11-21
gap: ORACLE
secondary: null
status: PARTIAL
summary: >-
  The two-peer test harness stopped its sync loop on "nothing imported, nothing pushed", which never held once
  the peers were paired, so every test ran exactly six rounds and the "sync fixed point" in its messages was only "after six rounds".
---

## Bug

Found while researching the intermittent red of
`two_instance_composed_pbt::owner_heavy_indent_then_join_stalls_the_receiver_projection` (known-red row
`owner-heavy-indent-join-receiver-not-converged`, 1 red in 13 runs on main). The red could not be reproduced:
0 of 80 runs on the lane base. Found by a lane research round reading the harness, not by an automated gate.

## Root cause

- `drive_to_sync_fixpoint` stopped when a round imported 0 and pushed 0. The relay hands the same envelope every round
  (pushed 1, imported 1, one audience refusal per round), so the rule never held and the loop always ran its bound of 6 rounds.
- A convergence verdict after a loop that always hits its bound says "after 6 rounds", not "at a fixed point".
  Whether 6 rounds is too few is not shown: the 0 of 80 green runs do not tie the 1 of 13 red to this hole.
- Pending (dependency-blocked) Loro imports are not visible to the harness, so a late import is not part of any stop rule.

## Missing piece

An oracle that the fixed point is real: a stop rule that is satisfiable, and a bound that fails loud instead of passing.
The convergence panic also carried no version vectors, so a red could not say whether a late import or a missing op was behind it.

## Remedy

Harness behaviour change (judgement of all 37 tests of the module changes):
- The loop stops when the oplog version vectors of both sides (global and layout doc) are unchanged over one two-way round,
  and panics `FIXPOINT-NOT-REACHED` after 6 rounds. The bound is never raised.
- The convergence panic carries both version vectors; a divergence prints `DIVERGENCE-EVIDENCE` lines to stderr.
- Open: the cause of the 1 of 13 red (unmeasured), the pending-imports gap, and the relay re-handing the same envelope each round.
