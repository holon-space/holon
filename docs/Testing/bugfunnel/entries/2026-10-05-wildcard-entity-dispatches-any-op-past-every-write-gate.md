---
id: 2026-10-05-wildcard-entity-dispatches-any-op-past-every-write-gate
date: 2026-10-05
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  Dispatching any operation under entity `*` ran it on whichever provider
  advertised that op name, past the boundary seam, the declared guard, the net
  gate and the write tier — so an MCP agent could delete or edit a block a
  named dispatch refused.
---

## Bug

Found by a verifier on the Inc 7 lane, not by an automated test. Deleting a
decision's only option was refused for entity `block` and **succeeded** for
entity `*`, same op name and same params. Probe source:
`/Users/martin/Workspaces/pkm/holon/.claude/worktrees/agent-a14bd2d6e1d15b82a/lane-logs/verify-inc7-r7/zz_verifier_probe.rs.kept`
(that probe uses the Inc 7 shape gate, which is not on `main`; the lane
reproduced the same bypass against the ADR 0032 net gate, which is).

`entity_name` is caller-supplied and unconstrained all the way from the MCP
tool surface: `frontends/mcp/src/types.rs` `ExecuteOperationParams.entity_name:
String` is passed verbatim at `frontends/mcp/src/tools.rs` `execute_operation`
(~line 2828) into `EntityName::new`, which parses `"*"` to
`EntityName::Wildcard`. An agent that typed `*` instead of `block` therefore
reached an unjudged write path.

## Root cause

`OperationDispatcher::execute_operation_with_provenance` branched on
`entity_name == "*"` and, after two special cases (`rebuild_views`,
`full_sync`), fell into a **generic** arm: collect every provider advertising
an op with that *name*, then call `provider.execute_operation` on each. That
arm ran none of the four gates the named arm runs — ADR 0028 boundary seam,
ADR 0031 declared guard, ADR 0032 net gate, write tier — nor the
entity-reference parse or the intent boundary.

The comment above the branch argued the gap was vacuous: the only ops
advertised under `*` are `sync`, `full_sync` and `rebuild_views`, all with no
subject param, and it warned that "a wildcard op that DOES take a subject
param would break that reasoning". The reasoning held for what the dispatcher
*advertises* and said nothing about what a caller may *pass* — and the arm
matched on op name alone, so `*` + `set_field` routed to the block provider.

Red log: `lane-logs/red-01.log` — the `block` dispatch is refused
(`net-guard refusal`), the `*` dispatch with identical params returns
`OperationResult { ... delivery: Proven }`.

## Missing piece

No invariant asserted that the gate chain is total over the dispatch surface.
Every gate is tested on its named arm only, so the whole wildcard arm was
outside the oracle (ORACLE). The keystone also generates `*` only with `sync` /
`full_sync` / `rebuild_views` (`net_totality.rs`, `settle_budget.rs`), so no
case ever paired `*` with a subject-taking op (COVERAGE).

## Remedy

Parse-don't-validate at the wildcard arm
(`crates/holon/src/api/operation_dispatcher.rs`):

- `BroadcastOp` is a closed enum (`Sync`, `FullSync`, `RebuildViews`) —
  exactly the three ops the dispatcher synthesizes under `*`.
  `BroadcastOp::parse` refuses any other name with `NotABroadcastOp`, which
  states why `*` carries no judgeable subject. The generic match-any-provider
  arm is gone, not disabled.
- Each per-provider fan-out call (`sync`, and `clear_cache` + `sync` under
  `full_sync`) now runs the same four gates a named dispatch runs, under the
  provider's own entity name. A gate refusal propagates; a provider's own
  failure stays counted, so one unreachable external system cannot stop the
  others.
- `*::rebuild_views` with no view rebuild wired was a caller-reachable
  `expect` panic; it is a named `Err` now.
- A sync-token clear that fails aborts `full_sync` instead of being logged
  and ignored.
- The op name is not the whole caller-supplied surface: a broadcast's **params**
  are judged too. `*` plus one of the three carrying `id` or `parent_id` is
  refused with `BroadcastCarriesASubject`. Those params do reach the gates on
  the fan-out, but bound to the descriptor of whichever provider advertises the
  op — written for an operation that touches no block — so the subject would be
  judged under declarations that were never about it. `SUBJECT_PARAM_KEYS` is
  the single list the write tier reads and this refusal enforces, so a gate
  that learns a new subject key cannot leave the wildcard arm behind.

Two further legs, from the same verifier's probes P3 and P6 on the fix itself
(`lane-logs/wildcard-gate-verify.md` § Defects D2 and D3). Both are the same
root cause one layer in: what the caller supplies — the op name, and WHEN the
judgement happens relative to the effect — decided an effect no gate had
judged.

- **A gate refusal is a refusal of the whole broadcast.** The fan-out judged
  each member as it reached it, so a refusal on provider N+1 left provider N
  already run: `*::full_sync` cleared every sync token, cleared one cache, ran
  no `sync` at all, and reported only the provider it stopped at. A broadcast
  has no inverse, so that state is unreachable from the error. `judge_fan_out`
  now judges every member and runs none; `JudgedFanOut::run` is the only way to
  run them, and holding one is the proof that no gate refuses. One refusal
  refuses the broadcast naming EVERY provider that refused, and `full_sync`
  judges both legs before the sync tokens — its first effect — are cleared.
- **A broadcast no provider can run is a named error with no effect.**
  `BroadcastOp::parse` accepted all three names unconditionally, while
  `operations()` advertised `sync` / `full_sync` only with a syncable provider
  and `rebuild_views` only with a view rebuild wired. On a dispatcher with no
  providers, `*::full_sync` cleared every sync token past all four gates and
  reported success. `OperationDispatcher::offered_broadcasts` is now the ONE
  place that decides, read by `operations()` (which advertises) and by the
  wildcard arm (which accepts); the arm reaches a broadcast only through an
  `OfferedBroadcast`, which carries the wiring that answers it, and anything
  else is `BroadcastNotOffered` naming the wiring it wanted.

Tests (all in `crates/holon/src/api/operation_dispatcher.rs`):
`the_wildcard_entity_carries_no_write_past_the_net_gate`,
`a_broadcast_fan_out_asks_the_gates_a_named_dispatch_asks`,
`the_broadcast_set_is_exactly_the_wildcard_ops_the_dispatcher_advertises`,
`every_broadcast_op_is_subjectless`,
`a_broadcast_whose_gates_refuse_runs_nothing`,
`a_broadcast_no_provider_can_run_is_refused_with_no_effect`, and
`the_wildcard_ops_a_dispatcher_advertises_are_exactly_the_ones_it_accepts`,
which replaces `every_advertised_wildcard_op_is_a_broadcast_op`: that one
checked advertised ⊆ broadcastable, the direction the drift went the other way
in. The new one compares the ops a composition ADVERTISES with the ones its
wildcard arm ACCEPTS, as sets, over four compositions (no provider, syncable,
view rebuild wired, both), each composition's advertised count measured so the
comparison cannot pass by both sides being empty.

Red logs for the three legs: `lane-logs/r3-red-02.log` (and
`lane-logs/r3-red-01.log` for the first run of the same tests); green
`lane-logs/r3-green-01.log`.

Teeth by inversion: a permissive `parse` reddens two of them
(`lane-logs/teeth-A.log`); a gateless `fan_out` reddens the third
(`lane-logs/teeth-B.log`). For `every_broadcast_op_is_subjectless`: dropping
the params check lets the subject reach the provider
(`lane-logs/subjectless-teeth-A.log`), a variant that declares a subject key
is refused (`lane-logs/subjectless-teeth-B.log`), and a new `BroadcastOp`
variant does not compile until it answers whether it names a subject
(`lane-logs/subjectless-teeth-C2.log`).

Keystone repro: not attempted as a keystone change in this lane. Closing the
COVERAGE half means letting the keystone generate `*` paired with a
subject-taking op; that generator extension is **open**.
