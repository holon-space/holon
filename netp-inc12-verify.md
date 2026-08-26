# netp-inc12 — adversarial verification

## Overall: REFUTED

Verified in pwd `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/netp-inc12`,
rev `wolovxzuvurx e95d7268d43b`. Tree identity passed (all four STEP-0 sentinels present; no MISS).

**Decisive defect:** the composed keystone (`just keystone-smoke`) is **RED**, caused by this
lane's own new `inv-net-totality` invariant. The report's §6 Gates was left "pending" and never
run; a red keystone was shipped. Root cause: the invariant's SUT cap in the frontend slice wires
the Increment-1 red-step BLOCK-ONLY net (`net_cap::derived_net_of`, built from
`engine.available_operations("block")` + empty rules) instead of the Increment-2 total catalog
`engine.derived_net()`. The composed keystone therefore never exercises Increment 2's production
source and reds whenever a non-block op fires.

---

## Per-claim

### 1 — ablation red-for-right-reason: CONFIRMED (unit level)
holon-integration-tests body tests 3/3 pass. `dropping_one_descriptor_registration_reds` returns
`Fail` naming `op:navigation.focus` (genuine miss, not compile/panic/unrelated assert).
`a_described_operation_passes_even_when_unanalyzable` + `the_wildcard_fan_out_is_not_a_hole` pass.
Key format `op:{entity}.{op}` (bridge.rs:36), no normalization. Body logic non-vacuous.
Caveat: the COMPOSED wiring of this same invariant is defective (see Claim 8).

### 2 — no prod behavior change from span: CONFIRMED
operation_dispatcher.rs diff = two additive lines only: `operation.resolved_entity =
tracing::field::Empty` (line 567) + `Span::current().record(...)` (line 835). `resolved_entity_name`
pre-exists (lines 805-834, not in diff). No control-flow change.

### 3 — ADR-0032 fidelity: CONFIRMED (a,b,c)
(a) §1 rewritten in place: written-marking durable, read-only place need only be observable,
narrow write-owes-durable exception. (b) §2 "Delivery is not authority", held net not a cache;
code `derived_net()` (backend_engine.rs:1292) recomputes per call, NO net cache field (only unrelated
graph_schema_cache). (c) §2 + Deferred item 7 record meta-transitions as foreseen. No ADR
over-claim vs code. (Report §2c calls engine.derived_net() the prod source, but the keystone cap
does not use it — see Claim 8; that is a code/report gap, not an ADR contradiction.)

### 4 — sub-fork: CONFIRMED, non-vacuous
`the_net_admits_instantiate_template_exactly_when_dispatch_does` PASS. Both
`firable_block_synthetic_descriptors` (operation_engine.rs:2272) and `has_operation`
(operation_engine.rs:2834) read the same `template_source.is_some()`. Test engine wires a template
source (sibling `advertised_in_available_operations` PASS), so dispatchable=true → equality
discriminates against registration's `false` literal.

### 5 — duplicate-first + wildcard: CONFIRMED
`derive_net` (compile.rs:82) Errs on any dup key; `operation_catalog` (backend_engine.rs:1264) dedups
first-wins and drops `*` before calling it. `the_wildcard_sync_descriptors_are_not_in_the_net` (with
non-empty guard) + `the_engine_synthetic_block_compounds_are_in_the_net` + holon-net
`two_descriptors_with_the_same_entity_and_op_are_refused` all PASS.

### 6 — derive cost: CONFIRMED
`deriving_the_net_is_cheap_enough_for_the_tick_path` PASS; real `<5ms` assert, not ignored/no-op.
Re-measured --no-capture: 106.549µs/call (report said 219µs; both far under bound).

### 7 — lock discipline: CONFIRMED
accepted_rules.rs std::sync::RwLock; set/clear/sources take+drop guard in one statement, return
owned data — guard never returned, cannot be held across await. All watcher call sites (lines
104-105,141-228) single-statement. `a_refusal_is_published_as_a_source_and_a_delete_forgets_it` PASS.

### 8 — gates: REFUTED
- gate-compile: PASS, both legs Finished, 0 errors (gate-compile.log:314,421).
- holon-net: 25/25 pass.
- holon --lib net filter: 33/33 pass.
- net_totality body: 3/3 pass.
- **keystone-smoke: FAILED (RED).** `test result: FAILED. 3 passed; 1 failed` (keystone-smoke.log:3434);
  `recipe keystone-smoke failed on line 143 with exit code 101` (line 3438). The background
  wrapper's "exit 0" was the `parallel | tee` pipe masking the real exit.

Failing invariant = inv-net-totality itself (keystone-smoke.log:97,115):
"the derived net does not describe 1 operation(s) this run fired: [op:navigation.focus] — the net
has 30 transition(s)". first-divergent-layer: store/CRDT (inv-net-totality). NOVEL failure, NOT the
registered known-red `sidebar-focus-bind` (that is a barrier timeout in await_sidebar_intent,
KeystoneKnownReds.md:115 — different mechanism). Not in KeystoneKnownReds.md. 100% lane-attributed
(net_totality.rs, net_cap.rs, cap impl all new here).

---

## Root cause (repro)

crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:1959-1960:
    async fn derived_net(&self) -> holon_net::CompiledNet {
        crate::pbt::net_cap::derived_net_of(&self.engine).await   // block-only RED-STEP net
    }
`net_cap::derived_net_of` is documented as the Increment-1 RED STEP: net from
`engine.available_operations("block")` (30 block-only transitions) + empty rules — NOT
`engine.derived_net()` (Increment-2 total catalog, which would include navigation.focus + all
provider ops). So the composed keystone checks fired ops against a block-only net; navigation.focus
fires in a normal composed run and the invariant reds.

Consequences:
1. Composed keystone red on a normal wiring — the gate the report never filled is failing.
2. Increment 2's production source (BackendEngine::derived_net()) is never exercised by the composed
   keystone; it still runs the Increment-1 stub.

Reproduce:
    cd /Users/martin/Workspaces/pkm/holon/.claude/worktrees/netp-inc12
    just keystone-smoke          # exit 101; inv-net-totality reds on op:navigation.focus
Minimal failing input at keystone-smoke.log:119 (journals Page block draw).

Logs: target/verify-logs/{gate-compile,holon-net,holon-lib-net,body,cost,keystone-smoke}.log
