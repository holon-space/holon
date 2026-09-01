# Adversarial verification — lane `reds-triage`

Verifier session, fresh context. Workspace
`/Users/martin/Workspaces/pkm/holon/.claude/worktrees/reds-triage`, `@-` =
`ed38a4dae833`. Toolchain asserted in-lane:
`nightly-2026-08-16-aarch64-apple-darwin (overridden by .../reds-triage/rust-toolchain.toml)`
(`lane-logs/verify/v-toolchain.txt`). No jj/git write commands. Every probe edit
was copied aside and written back, sha256 shown.

**Overall verdict: CONFIRMED with qualifications.** The ALPN fix is real and
proven by my own inversion; every number in the lane report reproduces. Three
qualifications below are defects in the lane's *claims*, not in its code: the
"product defect" framing, a net coverage loss in group E, and a
non-idempotent gate.

## My own runs

| Log | Summary line |
|---|---|
| `lane-logs/verify/v-full.nextest.txt` | `Summary [ 239.051s] 689 tests run: 682 passed (6 slow), 7 failed, 5 skipped`; `real 244.10` |
| `lane-logs/verify/v-invert.nextest.txt` | `Summary [  31.527s] 48 tests run: 33 passed, 15 failed, 0 skipped` |
| `lane-logs/verify/v-jsonprobe.nextest.txt` | `Summary [  14.045s] 8 tests run: 7 passed, 1 failed, 0 skipped` |
| `lane-logs/verify/v-latency-iso2.txt` | 3 isolated runs, see claim 6 |
| `lane-logs/verify/v-gates.*` | all gates, see claim 8 |

---

## 1. Tree identity — CONFIRMED

`crates/holon-loro/src/iroh_sync_adapter.rs:531` on disk inside `accept_sync`:

```rust
self.endpoint.set_alpns(vec![self.alpn(doc_id)]);
```

sha256 `0c4937ed4bc5a93b9208fc39860bc8517aaf7be89d3aa88f4a797eee3bba1215`,
matching the lane report. Not `wrong-tree`.

## 2. "Zero-ALPN endpoint / zero callers / real product defect" — PARTIALLY REFUTED

Confirmed by my own greps:

- `new_with_alpns` — **one** occurrence workspace-wide, its own definition at
  `iroh_sync_adapter.rs:469`. Zero callers. CONFIRMED.
- `IrohSyncAdapter::new` bound with `Endpoint::builder().bind()` (line 462), no
  ALPNs. The path never worked — proven by claim 3's inversion.

**REFUTED: "real product defect".** Every `IrohSyncAdapter` call site in the
workspace is a test or an example:

- `crates/holon/tests/{integration_tests,reliability_tests,stress_tests}.rs`
- `examples/peer_discovery.rs:49`

No `crates/**/src/**` file constructs it. Production sharing goes through a
different transport: `holon-sharing/src/sync.rs` drives
`holon_loro::sync_transport::SyncTransport`, and the iroh leg is
`crates/holon-loro/src/iroh_advertiser.rs:151,154`, which calls
`create_endpoint_with_key(vec![alpn], …)` / `create_endpoint(vec![alpn])` —
ALPNs registered at bind, correctly. `crates/holon-loro/src/loro_backend.rs:4393`
states it outright: `"P2P sync requires IrohSyncAdapter (not wired to LoroBackend)"`.

So the honest classification is **dead / test-and-example-only path**, not a
shipping product bug. The lane report concedes this in Open Question 2 ("It has
no production callers"), but the deliverable
`docs/Testing/HolonCrateReds-2026-09-01.md` marks all 15 rows `REAL-DEFECT`
(lines 35, 46-48) and the section heading is "the ALPN defect"; the word
"production" does not appear anywhere in that document. A reader of the
deliverable alone would conclude Holon's shipping sync was broken. That is the
defect: the deliverable and the report disagree, and the deliverable is the
artifact that survives.

## 3. Inversion — CONFIRMED

Removed **only** line 531 (`diff` showed exactly `531d530`), left everything
else including the reliability/stress test edits in place, then ran the three
binaries:

`lane-logs/verify/v-invert.nextest.txt` — `Summary [ 31.527s] 48 tests run:
33 passed, 15 failed, 0 skipped`. Exactly 15 red, matching the lane's A+B set:
9 × `integration_tests`, 3 × `reliability_tests`, 3 × `stress_tests` (including
`test_parallel_sync_operations`, FAIL at 30.757s).

Restore proved byte-for-byte:
`0c4937ed4bc5a93b9208fc39860bc8517aaf7be89d3aa88f4a797eee3bba1215` — identical
to the pre-probe hash.

## 4. Oracle changes — MIXED

**`test_alpn_mismatch_detection` — still vacuous, but honestly disclosed.**
My inversion run shows `PASS [ 0.771s] (29/48)` with the ALPN fix *removed*, and
`PASS [ 1.230s]` with it present. The test is insensitive to whether ALPN
registration works at all: it only asserts `accept_result.is_err()` and accepts
the substring `"protocol"`, which the zero-ALPN handshake failure also produced.
It has never proven the `"Wrong document"` bail path it is named for, and still
does not. The lane did NOT hide this — the deliverable's "Two vacuous passes
this uncovered" section states it explicitly. Claim honest; test worthless.

**`test_sync_with_empty_peer` — non-vacuous.** `assert_eq!(text2, "")` →
`assert_eq!(text2, "Non-empty")`. The new oracle asserts real content crossed
the wire; it went red in my inversion (`FAIL [ 0.580s] (35/48)`). The *old*
oracle was the vacuous one. Correct change.

**`test_parallel_sync_operations` — yes, and fast.** It now accepts on
`hub_adapter`, the adapter whose `addr()` it published to the clients
(`stress_tests.rs:193-204`), five accepts sequentially in one task. My full run:
`PASS [ 3.280s] (433/689)`. It has teeth — `accept_handle.await??` propagates
any accept error, and it went red at 30.757s under inversion.

Residual (pre-existing, not introduced): the final assertion
`assert!(!hub_text.is_empty())` is vacuous — the hub was seeded with
`"Hub content"` before any sync, so it holds even if all five clients fail.
Client failures are swallowed (`.ok()?` at line 215, `let _ = handle.await`).
Nothing asserts any client's content reached the hub. Also, unlike its
neighbours it carries no `#[serial]`.

## 5. Group E result-level assertions — PARTIALLY REFUTED

Two of three are strong; one lost coverage.

- `test_union_query_with_json_object_via_backend_engine` — strong:
  `assert_eq!(results.len(), 4)` plus a per-row `entity_name` check. Only the
  fixture gained `_change_origin`.
- `test_printf_sql_issue` — strong: asserts the exact value
  `Value::String("000000000100")` and `results.len() == 1`.
- `test_json_aggregation_includes_derived_columns` — **teeth confirmed but
  coverage reduced.** Probe: changed line 490 to
  `row.get("entity_name_PROBE_BROKEN")`. Result:
  `FAIL [ 1.699s] (2/8) test_json_aggregation_includes_derived_columns`
  (`lane-logs/verify/v-jsonprobe.nextest.txt`), the other 7 green. So the loop
  really executes and the `entity_name` assertion is real. Restored; sha256
  `c9ac56617522a253caab70f5b15e80a490ae8f2e306d01a4dc14a421c4469d5c` matches.

  **Defect:** the deleted SQL-text assertions covered **two** derived columns,
  `entity_name` *and* `display_name`. Only `entity_name` has a result-level
  replacement. `display_name` now has no assertion anywhere in that test — the
  probe log line 107 shows it is present at runtime
  (`Row 0: keys=["display_name", "id", "name", "entity_name"]`), so behaviour is
  fine, but nothing would catch its loss. Secondary: there is no
  `assert!(!results.is_empty())` before the loop, so an empty result set makes
  the whole test pass with zero assertions executed.

## 6. Census and load-sensitivity — CONFIRMED, with a thinner margin than reported

**Exactly 5 matview reds.** My run:
`grep -c "cannot modify materialized view block"` = **5**, on
`test_create_and_delete_workflow`, `test_basic_query_execution`,
`test_multiple_operations_sequence`, `test_query_and_watch_stream`,
`test_operation_triggers_stream_update`. No sixth.

**Load-sensitivity confirmed.** Contended, in the lane's 885-test census
(`lane-logs/ab-holon-main.nextest.log:1164`):
`[delivery] create+first-read = 7.516280208s (35 rows)` → FAIL at 8.245s.
Lane's isolated (`lane-logs/latency-iso.txt`): `859.623542ms`, `869.246416ms`,
`853.700375ms`.

My own isolated re-measure (`lane-logs/verify/v-latency-iso2.txt`), machine load
average ~23-25: `870.459042ms`, `945.110667ms`, `872.418ms` — the lane's numbers
reproduce.

**Caveat the lane understates.** An earlier single isolated run of mine
(`lane-logs/verify/v-latency-iso.txt`), taken at load average **46.07**, measured
`[delivery] create+first-read = 4.73778425s` — a PASS, but within 5% of the hard
`Duration::from_secs(5)` budget at
`crates/holon/tests/turso_storage_repros/tabs_main_panel_delivery.rs:183`, while
running as the *only* test in the process. The guard is not merely
"fails under an 885-test run"; it is near the cliff on a busy machine even
alone. Whatever concurrency pin the report proposes must account for machine
load, not only for in-run test parallelism.

## 7. The remaining 7 — CONFIRMED

My own `cargo nextest run --no-fail-fast -p holon -p holon-app`
(`lane-logs/verify/v-full.nextest.txt`):

`Summary [ 239.051s] 689 tests run: 682 passed (6 slow), 7 failed, 5 skipped`,
wall clock `real 244.10`. (689, not the lane's 884, because the brief's command
omits `-p holon-kitchen -p holon-core`.)

The 7, each independently classified:

1. `e2e_backend_engine_test test_create_and_delete_workflow` — matview known red
2. `e2e_backend_engine_test test_basic_query_execution` — matview known red
3. `e2e_backend_engine_test test_multiple_operations_sequence` — matview known red
4. `e2e_backend_engine_test test_query_and_watch_stream` — matview known red
5. `e2e_backend_engine_test test_operation_triggers_stream_update` — matview known red
   (all five: `Failed to prepare statement: Parse error: cannot modify materialized view block`)
6. `turso_storage_pbt pbt_tests::tests::test_turso_backend_state_machine` —
   group C dead-suite: `Failed to get version from Turso:
   DatabaseError("Failed to prepare query: Parse error: no such column: _version")`
   (`pbt_tests.rs:1343`)
7. `create_page_from_link recreating_a_renamed_pages_old_name_yields_a_distinct_page`
   — group D pending feature; the panic message itself cites it:
   `"recreating page A must succeed (§5.3). Interim ADR 0029 D1b refuses it with
   IdentityCollision instead; the end-state unique-random recreate is not implemented"`
   (`create_page_from_link.rs:641`)

Nothing outside the three sanctioned classes. The only `^error:` line in the log
is nextest's own `error: test run failed`.

## 8. Gates — CONFIRMED, but one gate is not idempotent

`lane-logs/verify/v-gates.sh`, outer exit `0`, all six steps reached
`### ALL GATES DONE`:

| Gate | Log | Result |
|---|---|---|
| `cargo fmt --all --check` | `v-fmt.txt` | 0 bytes, clean |
| `cargo check -p holon-gpui -p holon-app` | `v-check.txt` | `Finished dev profile ... in 21.43s`, 0 errors |
| `just keystone-smoke` | `v-keystone.txt` | `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.18s` |
| `/usr/bin/python3 scripts/bugfunnel.py check` | `v-bugfunnel.txt` | `586 entries, 0 problems` |
| `just check-worker-wasm` | `v-workerwasm.txt` | `test result: ok. 5 passed; 0 failed; ...` |

**Defect — the worker lockfile hazard is live, not merely historical.** Before
my gates, `jj status` showed exactly the 7 intended files. After
`just check-worker-wasm`, `jj status` gained
`M frontends/holon-worker/Cargo.lock` at sha256
`6583a3ac18495d6329dd931b0db77c23358fa182240105c726d6bb6996c3bb61` — the *same*
hash the lane report quotes as its "before restore" value. I restored it from
`@-` (`jj file show -r ed38a4da`), back to
`db433c23a7e4b0128745bd260a20ad94de22af1b7346abac61c8656cbdaee257`; `jj status`
now again shows only the 7 intended files.

Consequence for the orchestrator: the lane's clean tree is a *hand-restored*
state, and `check-worker-wasm` is one of the gates the weave will run. Anyone
running it before landing must restore the lock again, or the weave picks up a
96-insertion lockfile churn that is not this lane's change. Fixing the stale
base lock at source (as the lane recommends) is the durable answer.

## Design note (not a claim under test, but introduced by this fix)

`Endpoint::set_alpns` **replaces** the server config rather than adding to it —
iroh 0.96.1 `src/endpoint.rs:757-760`:
`self.sock.endpoint().set_server_config(Some(server_config))`. So
`accept_sync(doc_b)` silently un-registers `doc_a`'s ALPN on the same adapter.
Two consequences: the legacy adapter cannot accept for two documents
concurrently on one endpoint (the fix makes this a live footgun where before
nothing worked at all), and `new_with_alpns`'s registration is destroyed by the
first `accept_sync`. The lane report flags the second in "Deliberately left
out"; the first is unflagged. Neither affects production, per claim 2.

## Style note

`crates/holon/tests/json_aggregation_e2e_test.rs:715-717` — the new doc comment
on `test_printf_sql_issue` describes history ("the forms callers adopted while
the engine still rejected the bare call"), which `CLAUDE.md` forbids
("Comments describe the current state of the code, never its history").

## Tree state at exit

`jj status` = the 7 intended files, unmodified from the lane's delivered state.
All probe files restored and hash-verified:

```
0c4937ed4bc5a93b9208fc39860bc8517aaf7be89d3aa88f4a797eee3bba1215  crates/holon-loro/src/iroh_sync_adapter.rs
c9ac56617522a253caab70f5b15e80a490ae8f2e306d01a4dc14a421c4469d5c  crates/holon/tests/json_aggregation_e2e_test.rs
bb4f9295b2989e011b142410cb0fceba4c1612b13104502d53266a30e4a4ee03  crates/holon/tests/stress_tests.rs
91f9c28d46456f0593068d4774019c82f1176ac3d96e5ee08da8a4b224c4bdef  crates/holon/tests/reliability_tests.rs
db433c23a7e4b0128745bd260a20ad94de22af1b7346abac61c8656cbdaee257  frontends/holon-worker/Cargo.lock
```

---

# Re-verification — 2026-09-02

Second adversarial pass over the revised delta (`@` = `1b1aa070`, `@-` =
`ed38a4dae833`). The delta now also touches `crates/holon/tests/integration_tests.rs`
(+122) and reworks `iroh_sync_adapter.rs` (+31). Same rules; every probe edit
copied aside and written back, sha256 shown. Pre-probe baseline:

```
bf0e03fdb593360af1066a331c483338ef5a67fda8724953081498a316cd9314  crates/holon-loro/src/iroh_sync_adapter.rs
cb6fda0dc8293f8ca7e9b3525319334d0eaf6bd3042ea8d534bdd5c3999fb6b5  crates/holon/tests/integration_tests.rs
1df73df8f418b01e87e56311fbce334c5671a5c81b91721dec08d531c905d8dd  crates/holon/tests/json_aggregation_e2e_test.rs
```

**Verdict: CONFIRMED.** All six claims hold. Every defect the first pass raised
is closed. One caveat on claim 6 (an eighth, sanctioned-flake failure) and two
minor notes, none blocking.

## R1. Cumulative ALPN registration — CONFIRMED

`register_alpn` (`iroh_sync_adapter.rs`) inserts into a
`Mutex<BTreeSet<Vec<u8>>>` and then calls
`self.endpoint.set_alpns(registered.iter().cloned().collect())` — the **full
set, every call**, never a single-element vector. `new_with_alpns` now seeds
that same set, so its registration survives the first `accept_sync` (the first
pass's second design note, closed).

Inversion: I reverted **only** the union, leaving the set bookkeeping intact —
`set_alpns(vec![self.alpn(doc_id)])` — and ran the `integration_tests` binary.

`lane-logs/verify/v2-union.nextest.txt`:
`Summary [ 15.083s] 18 tests run: 17 passed, 1 failed, 0 skipped`

The single failure is the new test, red for exactly the named reason:

```
FAIL [ 0.562s] ( 1/18) holon::integration_tests accepting_a_second_doc_keeps_the_first_docs_alpn_registered
cumulative-a was refused at the handshake — its ALPN was un-registered:
  aborted by peer: the cryptographic handshake failed: error 120: peer doesn't support any known protocol
```

Nothing else moved, so the union is doing exactly the work claimed. Restored;
sha256 `bf0e03fd…` matches the baseline.

## R2. `test_alpn_mismatch_detection` now has teeth — CONFIRMED

First pass: the test passed *with and without* the fix. Now, with
`self.register_alpn(doc_id);` deleted from `accept_sync`
(`lane-logs/verify/v2-invert.nextest.txt`):

`Summary [ 31.396s] 49 tests run: 32 passed, 17 failed, 0 skipped`, including
`FAIL [ 1.211s] (38/49) holon::integration_tests test_alpn_mismatch_detection`.

It fails through the newly added **leg 2** (the positive path): the
`dialer.sync_with_peer(&dialled, addr).await?` returns
`aborted by peer: … error 120: peer doesn't support any known protocol`. Leg 1
(the refusal) still passes vacuously on its own, exactly as the test's own doc
comment now says — leg 2 is what supplies the teeth. Correct design.

The other 16 reds under inversion are the known 15 A+B set plus the new two-doc
test. Restored; sha256 matches.

## R3. json assertions have teeth — CONFIRMED (probed separately)

Breaking both at once would short-circuit on the length check, so I probed them
one at a time.

- `display_name` → `display_name_PROBE_BROKEN`: `lane-logs/verify/v2-json.nextest.txt`
  `Summary [ 1.179s] 8 tests run: 7 passed, 1 failed` — panic at
  `json_aggregation_e2e_test.rs:497`, `Row should have display_name from
  flattened data`. The log also shows the length check passed first, and the
  real row keys: `Row 0: keys=["id", "entity_name", "name", "display_name"]`.
- `assert_eq!(results.len(), 2)` → `99`: `lane-logs/verify/v2-json-len.nextest.txt`
  `Summary [ 1.003s] 8 tests run: 7 passed, 1 failed` — panic at
  `json_aggregation_e2e_test.rs:488`, `left: 2  right: 99`.

Both first-pass gaps (no `display_name` oracle; a loop that could pass with zero
assertions on an empty result set) are closed. Restored; sha256
`1df73df8…` matches.

## R4. Deliverable classification — CONFIRMED

All 13 ALPN-caused A+B rows (1-10, 12-14) now read `DEAD/TEST-ONLY PATH`. Rows
11 and 15 read `STALE-ORACLE`, which is correct — they were oracle/topology
fixes, not ALPN victims. **No row reads `REAL-DEFECT`.**

The section blockquote (lines 71-78) names production's transport:

> **This was never a shipping bug.** `IrohSyncAdapter` has **zero production
> callers** … Production sharing goes through `SyncTransport`, whose iroh leg
> (`crates/holon-loro/src/iroh_advertiser.rs:151,154`) registers ALPNs correctly
> at bind; `crates/holon-loro/src/loro_backend.rs:4393` says so outright.

The only remaining `REAL-DEFECT` string is line 225, inside the dated changelog
recording the correction. Minor nit: it is phrased in the present tense ("The 15
A+B rows read `REAL-DEFECT`") where it means the prior state; a reader skimming
the changelog could mis-read it. Cosmetic only — the table itself is
unambiguous.

## R5. Tree state — CONFIRMED

`jj status` lists only intended files: the 5 modified sources, the 3 new docs,
and `reds-triage-verify.md` (this report). `jj diff -r @ -- frontends/holon-worker/Cargo.lock`
is **empty**; the file is byte-identical to base,
`db433c23a7e4b0128745bd260a20ad94de22af1b7346abac61c8656cbdaee257`. I did not
run `just check-worker-wasm` this pass, so the first pass's finding stands
unchanged: that gate re-dirties the lock every time it runs, and whoever runs it
before landing must restore it again.

`cargo fmt --all --check` clean (`lane-logs/verify/v2-fmt.txt`, 0 bytes).

## R6. Full run — CONFIRMED for the 7 durable reds; an 8th flake fired

`cargo nextest run --no-fail-fast -p holon -p holon-app`
(`lane-logs/verify/v2-full.nextest.txt`):

`Summary [ 177.973s] 690 tests run: 682 passed (5 slow), 8 failed, 5 skipped`,
wall clock `real 192.26`.

Failures:

1. `e2e_backend_engine_test test_create_and_delete_workflow` — matview known red
2. `e2e_backend_engine_test test_basic_query_execution` — matview known red
3. `e2e_backend_engine_test test_multiple_operations_sequence` — matview known red
4. `e2e_backend_engine_test test_query_and_watch_stream` — matview known red
5. `e2e_backend_engine_test test_operation_triggers_stream_update` — matview known red
6. `turso_storage_pbt pbt_tests::tests::test_turso_backend_state_machine` — group C dead-suite
7. `create_page_from_link recreating_a_renamed_pages_old_name_yields_a_distinct_page` — group D, ADR 0029 D1b
8. `sync_suite sync_pbt::tests::share_subtree_pbt::subtree_share_round_trip_pbt` — **flake**

The expected 7 reproduce exactly. The 8th is the flake named in the lane rules'
pass-with-note list. It is unrelated to this delta: it fails on a
filesystem-timing property, `sync_pbt.rs:803` —
`P-NO-TMP-LEFTOVER/B: stale tmp files: [".../shares/….loro.tmp"]` — nothing to do
with ALPNs or the adapter, which `sync_suite` does not use. It did not fire in my
first-pass full run, and it is green 3/3 on isolated re-runs
(`lane-logs/verify/v2-flake.txt`: PASS at 137.966s, 231.835s, 152.849s).

Process gap worth noting: this flake is named in the lane brief's pass-with-note
list but `grep` for `subtree_share_round_trip` and `NO-TMP-LEFTOVER` in
`docs/Testing/KeystoneKnownReds.md` returns nothing. If `-p holon` becomes a
per-weave gate as the deliverable recommends, this flake will red the gate with
no in-repo record explaining it.

## Notes on the new test (non-blocking)

`accepting_a_second_doc_keeps_the_first_docs_alpn_registered` asserts only
inside `if let Err(e)`, and tolerates any error that is not the handshake
refusal. That is the right shape for a racy two-accept setup — a successful dial
*is* the property holding — and probe R1 proves it goes red when the property
breaks. Both accept tasks are `abort()`ed rather than awaited, so accept-side
errors are not surfaced; the test's comment says as much.

## Files restored, final hashes

```
bf0e03fdb593360af1066a331c483338ef5a67fda8724953081498a316cd9314  crates/holon-loro/src/iroh_sync_adapter.rs
cb6fda0dc8293f8ca7e9b3525319334d0eaf6bd3042ea8d534bdd5c3999fb6b5  crates/holon/tests/integration_tests.rs
1df73df8f418b01e87e56311fbce334c5671a5c81b91721dec08d531c905d8dd  crates/holon/tests/json_aggregation_e2e_test.rs
db433c23a7e4b0128745bd260a20ad94de22af1b7346abac61c8656cbdaee257  frontends/holon-worker/Cargo.lock
```

---

# Third-round re-verification — 2026-09-02 (D65.a / D66.a / test-group)

`@` = `48b59272`, `@-` = `0b1c8df4` (the lane's round-2 commit), chain reaches
`ed38a4dae833`. Delta: 22 files, +127/−1400. Base caller counts were measured
against a tree extracted with `git -C <primary repo> archive 0b1c8df4dd9c`
(sentinel-asserted non-empty), per the jj-workspace archive hazard.

**Verdict: two claims REFUTED — the lane cannot land as-is.** `cargo fmt
--all --check` fails, and a deleted API is still documented as implemented.
Everything else holds; the deletion itself is clean.

## T1. Production sharing unaffected — CONFIRMED

Mechanical public-surface diff of the kept file (`grep -oE "pub (async fn|fn|struct|enum|trait|use) …"`,
base vs now). **Removed — 9 items, all `IrohSyncAdapter`'s:**

| Removed pub item | prod call sites at base | test/example call sites at base |
|---|---|---|
| `pub struct IrohSyncAdapter` | 0 | 54 name hits |
| `pub async fn new` | 0 | (of the 54) |
| `pub async fn new_with_alpns` | **0** | **0** |
| `pub fn set_peer_id_from_node` | 0 | 1 |
| `pub fn addr` | 0 | (of the 54) |
| `pub fn endpoint` | 0 | (of the 54) |
| `pub async fn sync_with_peer` | 0 | 26 |
| `pub async fn accept_sync` | 0 | 28 |
| `pub use adapter::IrohSyncAdapter` | re-export only | — |

Plus `pub use iroh_sync_adapter::IrohSyncAdapter` in `lib.rs`. **Added: none.**

The 8 apparent "production" `IrohSyncAdapter` hits at base are all doc-comments
plus the re-export (`loro_document.rs:122,256`, `deleted_container_purge.rs:321`,
`import_atomicity_probe.rs:12`, `loro_document_store.rs:209`,
`loro_backend.rs:4393` ×2, `lib.rs:163`) — **zero call sites**; every one is
updated or removed by this delta. The 27 "production" `sync_with_peer` hits are
`apply_sync_with_peer`, an unrelated PBT capability method (name collision),
untouched.

Kept and still exported: `create_endpoint`, `create_endpoint_with_key`,
`make_alpn`, `sync_doc_initiate`, `sync_doc_initiate_enrolled`,
`sync_doc_accept`, `sync_doc_handle_connection`, `connection_remote_addr`,
`IrohSync`, `SharedTreeSyncManager`, `SyncBackend`, `DirectSync`.
`cargo check --workspace --all-targets` compiles with **0 errors**
(`lane-logs/verify/v3-check.txt`, `Finished dev profile … in 29.73s`), so
`iroh_advertiser.rs` and `loro_share_backend.rs` still build against them. No
production path lost a function.

## T2. The 28 kept Loro-only tests still run — CONFIRMED

Counted from my own full run (`lane-logs/verify/v3-full.nextest.txt`):
`integration_tests` 7, `reliability_tests` 16, `stress_tests` 5 = **28**, all
PASS. Every name is Loro-document-only — convergence, peer-id, snapshot, utf8,
boundary/invalid positions, idempotency, isolation, stability, memory — none
touches a P2P endpoint. No adapter-driven test survived, and no Loro-only test
was collaterally deleted.

## T3. Residual references — REFUTED

The brief's pattern is too broad to be diagnostic (`_version\b` alone matches
426 lines: `schema_version`, `get_current_version`, `picked_items_version`,
`change_version`, `RESPONSE_VERSION_KEY`, `model_version`, `cli_version` — all
unrelated and untouched). Targeted greps on the actually-deleted identifiers:

- `IrohSyncAdapter` — 2 hits, both current-state docs (the deliverable and the
  bugfunnel entry, describing the fixed/deleted state). OK.
- `new_with_alpns` — 2 hits, same two docs. OK.
- `SetVersion` — 1 hit, the deliverable. OK. `"_dirty"` — **0**. OK.
- `peer_discovery` — 1 hit, the deliverable. OK.

**The refutation — `docs/Architecture/Storage.md:236`:**

```
- `get_version/set_version` - Optimistic locking support via `_version` column
  (implemented; currently exercised only by the storage PBTs, not wired into
  any production write path)
```

That is a present-tense "implemented" claim about an API this delta deleted. It
is neither unrelated nor current-state. The lane updated four architecture docs
(`Architecture.md`, `Architecture/Sync.md`, `Architecture/FeatureMap.md`,
`Architecture/BlockEventStorm.md`) and missed this one — the single doc that
described the *storage* half of D65.a. Secondary, lower severity:
`crates/holon/src/storage/TODO.md:140` still records
`✅ Updated get_version() and set_version() to use prepared statements`.

## T4. The nextest group binds to the right test — CONFIRMED

`cargo nextest show-config test-groups -p holon -p holon-app`
(`lane-logs/verify/v3-testgroups.txt`) resolves the override to exactly one test:

```
group: vault-scale-latency (max threads = 1)
  * override for default profile with filter 'test(cursor_filtered_main_panel_delivers_at_vault_scale)':
      holon::turso_storage_repros:
          tabs_main_panel_delivery::cursor_filtered_main_panel_delivers_at_vault_scale
```

The `#[ignore]` reason names the ADR verbatim
(`create_page_from_link.rs:602`):
`#[ignore = "ADR 0029 D1b end-state pending: unique-random recreate not implemented"]`.
Skipped count moved 5 → 6 in the full run, consistent.

## T5. `concurrent_keystrokes_keep_every_undo_step` — NOT REPRODUCED; classification DISPUTED

6/6 green for me: 5 isolated runs (`lane-logs/verify/v3-undo.txt` — PASS at
1.894s, 1.929s, 1.878s, 1.942s, 1.888s, load average 5.7–6.6) and 1 pass inside
the full `-p holon -p holon-app` run. I cannot reproduce the 7-vs-3 failure.

**It is not load-sensitive "like row 19", and should not be treated that way.**
Row 19's oracle is a wall-clock budget — a slower machine legitimately means a
slower number, so pinning concurrency removes noise without removing signal.
This test's oracle is a *correctness equality*: the undo walk after concurrent
typing must equal the walk after sequential typing
(`undo_concurrent_keystrokes.rs:203`). Its subject **is** concurrency —
`type_word_as_the_editor_does` deliberately spawns un-awaited writes 1 ms apart
to provoke interleavings. A differing step count under load is therefore either
a real race in the inverse-command log or an over-specified oracle; in neither
case is a `max-threads = 1` group the answer, because that pin would delete the
only condition under which the property is exercised at all. Recommend a
bugfunnel entry plus a high-repeat run (the failing step counts are the lead),
not a latency-style group.

## T6. Gates — fmt REFUTED, the rest CONFIRMED

**`cargo fmt --all --check` FAILS** (`lane-logs/verify/v3-fmt.clean.txt`), one
file, introduced by this delta's comment rewrite —
`crates/holon-loro/src/loro_document.rs:253`:

```
-    /// incremental delta; `export_delta_or_full_snapshot` detects that and ships a full
-    /// snapshot instead.
+    /// incremental delta; `export_delta_or_full_snapshot` detects that and
+    /// ships a full snapshot instead.
```

Renaming `IrohSyncAdapter` → `export_delta_or_full_snapshot` in that doc comment
pushed the line past the width limit and it was not re-wrapped. This is a hard
land-gate failure; `set -euo pipefail` aborted my gate script on it, so I re-ran
the remainder separately.

- `cargo check --workspace --all-targets` — **0 errors**, warnings only
  (pre-existing `unused_must_use` in `holon-filesystem`).
- Full run (`lane-logs/verify/v3-full.nextest.txt`):
  `Summary [ 243.654s] 687 tests run: 681 passed (5 slow), 6 failed, 6 skipped`,
  `real 246.83`. Failures:
  1-5. the five `e2e_backend_engine_test` matview known reds
  (`test_create_and_delete_workflow`, `test_basic_query_execution`,
  `test_multiple_operations_sequence`, `test_operation_triggers_stream_update`,
  `test_query_and_watch_stream`);
  6. `sync_suite … subtree_share_round_trip_pbt` — the known flake, its **second
  occurrence in three full runs** this session (green 3/3 isolated in round 2).
  Both previously-remaining reds are gone: the `_version` dead suite is deleted,
  the ADR 0029 D1b pin is ignored.
- `jj status` — only the 22 intended files. `frontends/holon-worker/Cargo.lock`
  is **not** in `jj diff -r @ --stat` (0 hits); I did not run
  `check-worker-wasm` this pass, so the round-1 finding about that gate
  re-dirtying it still stands unaddressed.

## Additional finding — leftover `#[cfg]` from the deletion

`crates/holon-loro/src/iroh_sync_adapter.rs:890-891` now carries two identical
attributes back to back:

```rust
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use adapter::SharedTreeSyncManager;
```

The deletion removed `pub use adapter::IrohSyncAdapter;` but not the `#[cfg]`
that guarded it. Rust ANDs stacked `cfg`s, so it compiles (confirmed by the
clean `cargo check`) — cosmetic cruft, but it is the visible trace of an
incomplete deletion and should go with the fmt fix.

## Blocking list for the orchestrator

1. `cargo fmt --all --check` red — `loro_document.rs:253`.
2. `docs/Architecture/Storage.md:236` documents deleted API as implemented.
3. (minor) duplicate `#[cfg]` at `iroh_sync_adapter.rs:890`.
4. (minor) `crates/holon/src/storage/TODO.md:140` stale reference.
5. (open, not this lane's) the `subtree_share_round_trip_pbt` flake rate.
