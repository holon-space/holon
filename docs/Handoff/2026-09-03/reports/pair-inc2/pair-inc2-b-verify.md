# Verify: lane `pair-inc2` increment B (D77.b) — CONFIRMED with two coverage findings

Fresh-context adversarial pass. WS `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/pair-inc2`,
uncommitted diff on `1cbf36d1`. Tree restored byte-identical afterwards
(`jj diff --stat` = 30 files, 849 insertions, 267 deletions, unchanged).
No jj/git write commands were run.

## Verdict

**CONFIRMED.** Every gate reproduced independently; the green criterion has real
teeth; the migration is correct under adversarial inputs. Two findings are
recorded below — neither refutes the implementation, both refute a claim about
what the tests *prove*.

## 1. Tree identity

- `pwd` = the lane WS on every call; toolchain `nightly-2026-08-16-aarch64-apple-darwin`.
- `rg -n get_global_doc crates frontends --glob '!target'` → **EMPTY**.
- `DocScope::Layout` present on disk in 7 files (loro, wiring, integration-tests).
- `DocScope { Global, Layout }` at `crates/holon-loro/src/loro_document_store.rs:34`;
  `DocKey<'a> { Global, Layout, Shared(&str) }` at `crates/holon-loro/src/loro_backend.rs:1947`.
- `is_live_anywhere` call sites in `block_cell_registry.rs`: lines 189, 315, 353,
  366, 492, 597, 616, 665, 728 = **9**, as claimed.
- Layout snapshot file is `holon_layout.loro` (`LAYOUT_SNAPSHOT_NAME`). Note: the
  global snapshot is `holon_tree.loro`, not `holon.loro` as the task brief said.

## 2. Gates (all reproduced in this session)

| Gate | Result | Log |
|---|---|---|
| `nextest -p holon-loro -p holon-app -p holon-core -p holon-architecture-tests --test-threads 4` | **645 run, 645 passed, 4 skipped** (48.7s) | `lane-logs/verify-nextest-1.log` |
| ↳ `loro_doc_escapes_match_the_allow_list` | PASS (line 249) | same |
| ↳ `archlint_all_passes` | PASS (line 457) | same |
| `cargo check --workspace --all-targets --features holon-integration-tests/pbt,holon-gpui/pbt` | exit 0 | `lane-logs/verify-check-1.log:2` |
| `cargo check -p holon-gpui --features holon-gpui/pbt` | clean | `:3` |
| `just check-worker-wasm` (wasm32-wasip1-threads) | clean | `:4` |
| `just check-frontend-wasm` (wasm32-unknown-unknown) | clean | `:213`, `ALL CHECK DONE` at `:261` |
| `just keystone-smoke` @ `PROPTEST_CASES=8`, run 1 | `4 passed; 0 failed` | `lane-logs/verify-keystone-1.log:159` |
| ↳ run 2 | `4 passed; 0 failed` | `:319` |
| `binary(two_instance_composed_pbt)` run 1 | **12 run, 12 passed, 5 skipped** | `lane-logs/verify-twoinst-1.log:256` |
| ↳ run 2 | 8 passed, 1 failed, 5 skipped | `:529` |
| `just analyze-arch` | `109 baselined suppressed, 0 new` | `lane-logs/verify-arch.log:1` |

Green criterion `the_device_local_layout_ids_resolve_to_one_live_node_after_a_round`
**PASSED in every one of the 6 runs I executed** (2 binary runs + 4 targeted runs).

### The one red, classified

Run 2's only failure is `one_way_share_converges_on_the_receiver_over_iroh`, at
`two_instance_composed_pbt.rs:271`:

```
the receiver's store converged but its ORG files are missing
[block:c1, block:c2, block:parent] — received state that never reaches disk is lost on restart
```

This is exactly the pre-existing **org write-back** class (pass-with-note). Nothing
else failed. It is load-sensitive in count, not in kind: run 1 had zero failures,
and a later 45-test sweep had this same single failure (non-iroh twin).

### Allow-list entries — each judged

`crates/holon-architecture-tests/tests/architecture_rules.rs:197,201,204`, three
counts raised, each carrying an inline `// ALLOW(loro_doc_escape)` at the new site:

- `block_cell_registry.rs` 1→2 — line 69, the layout doc's retained container
  handle, verbatim the same cell-backing rationale as line 65's global one. **OK.**
- `loro_backend.rs` 8→10 — lines 2199 and 3779, both commented "re-wrapped under
  the same boundary lock". Both re-wrap an existing `Arc<LoroDoc>` rather than
  handing a raw doc across the boundary. **OK.**
- `loro_sync_controller.rs` 1→2 — line 386, `subscribe_root` registration, the
  rationale the rule already admits for the global doc. **OK.**

No new raw reader escape. Verified by reading each site, not by trusting the counts.

## 3. Teeth

### (a) Does the green criterion actually detect layout doubling? — YES, but not via `ContainerRegistry`

The tooth as briefed **does not bite**, and that is a finding about the claim's
stated mechanism, not about the code:

1. I added the layout doc to `ContainerRegistry::replication_set` as an extra
   registered container. Test **still PASSED** (`lane-logs/verify-tooth-a.log`).
2. I proved the path *is* executed — an `eprintln!` in my inserted entry fires
   **24 times** under `--no-capture` (`lane-logs/verify-tooth-a3.log`) — and the
   test still passed.
3. Reason: the green criterion shares by container **name**, not by the
   replication set — `two_instance_composed_pbt.rs:1232` calls
   `two.share_container("holon_tree", "receiver")`. And
   `two_instance_transport.rs:369-372` asserts the iroh leg replicates the root
   container only. So `replicate_all`'s membership is not what this test observes.

So the report's headline framing — "never registered in `ContainerRegistry` so
`replicate_all` cannot reach it" — is a true statement about the code but is
**not the property this test pins**. What the test pins is *which doc holds the
layout*. I confirmed that with correct teeth:

| Tooth | Result |
|---|---|
| Collapse `DocScope::Layout` → `&self.global_doc` in `doc_slot` | **RED** — migration fail-loud fires: `layout migration refused: 25 block(s) are live in BOTH...` (`verify-tooth-a4.log:274`) |
| Disable only the layout-ROOT create routing (`if false && ...`) | PASS — the boot migration compensates by moving the subtree back out (`verify-tooth-a5.log`) |
| Disable the layout-ROOT routing **and** the migration | **RED, for the right reason**: `owner: 25 device-local layout id(s) resolve to MORE than one live Loro node after a round` (`verify-tooth-a6.log:267`) — verbatim the increment-A signature |

The criterion is genuine: 25 ids, and its own `layout.len() > 1` guard holds.
Routing and migration are jointly load-bearing and mutually redundant.

### (b) Reverting the `is_live_anywhere` guard — FINDING: nothing catches it

I reverted the create-idempotence guard at ONE site,
`crates/holon-loro/src/block_cell_registry.rs:366`, from
`backend.is_live_anywhere(new_id.id()).await` back to the global-only question
`backend.resolve_to_tree_id(new_id.id()).await.is_some()`.

- Green criterion: **still PASSES** (`verify-tooth-b.log`).
- Broad sweep with the tooth in place (`verify-tooth-b2.log`):
  `-p holon-loro -p holon-app` → **490 run, 490 passed**;
  `two_instance_composed_pbt` + `loro_suite` + `boot_suite` → 45 run, 44 passed,
  **1 failed = `one_way_share_converges_on_the_receiver`**, i.e. only the
  pre-existing org write-back red.

**No test names this regression.** The claim that "the migration's fail-loud arm
or the boot re-mint must show" is **not reproducible**: I could not name a test,
because none fails. The fix is right (the fail-loud arm did fire once during
development, per `lane-logs/incB-two-instance-1.log:1301`), but it is now
**unguarded by the suite** — a re-mint of the whole bundled layout on every boot
would land green. This is a coverage gap worth a bugfunnel/regression-test entry.

All mutated files restored by `cp` from backups and verified by sha256
(`container_registry.rs eb5cf2d2…`, `block_cell_registry.rs b10b9d40…`,
`loro_document_store.rs 086f0142…`, `loro_backend.rs 14d50aff…`,
`layout_migration.rs 7de6b698…`, `archlint/baseline.txt 17ecc372…`). No `jj restore`.

## 4. Migration, adversarial — CONFIRMED

`crates/holon-loro/src/layout_migration.rs` ships with **zero tests** (no `mod
tests`, no test file; its only other reference is the call at
`crates/holon-loro-wiring/src/loro_module.rs:262`). I wrote a scratch integration
test, ran it, and deleted it (`lane-logs/verify-scratch-mig3.log`, **3 passed**):

| Scenario | Expected | Observed |
|---|---|---|
| Legacy single-doc store: global holds `block:__default__` → `block:panel` → `block:panel::render::0`, plus an unrelated block; no layout doc | 3 moved; layout doc holds exactly the closure; global keeps only the unrelated block | **exactly that** |
| Same store, second boot | 0 moved, no writes, layout still 3 | **0, idempotent** |
| `block:__default__` live in BOTH docs | `Err`, no fallback, no merge | **`Err` containing "live in BOTH"** |
| Store with no layout at all | 0 moved, layout doc empty, global untouched | **exactly that** |

The `Vec<Block>` closure is parents-before-children and sibling groups sort on
`(sort_key, id)`; deletes run leaves-first (`.rev()`), so no parent delete can
take a child with it.

**SqlOnly / crdt disabled:** the layout doc is unconditional — `layout_doc` is a
plain field of `LoroDocumentStore`, `DocScope::Layout` has no `cfg` and no
feature gate anywhere on disk, and the wasm/worker/frontend targets both compile.
Confirmed structurally rather than by booting a SqlOnly vault.

## 5. Straddle — CONFIRMED (with a framing correction)

`DocKey` derives `PartialEq`, so `DocKey::Layout != DocKey::Global` by
construction — the old `Option<&str>` encoding could not have distinguished them.
The rejections all compare on it and all return a loud `Err`:

- re-parent: `loro_backend.rs:3351`
- positioned move: `:3426`
- the third move path: `:4527`
- batch create straddle: `:4666-4670`, `"create_blocks: batch straddles two docs
  ({:?} vs {:?}); cross-doc …"`

Correction to the check as briefed: a *create* whose parent lives in the layout
doc is **routed** to the layout doc, not rejected — that is the design
(`resolve_write_target_for_parent`, `loro_backend.rs:2494-2515`: layout-root by
child id, then `resolve_layout(parent)`). There is no "global-doc context" a
create can be issued from; the parent determines the doc. Straddling is only
reachable via a move or a multi-block batch, and both are rejected loudly. No
cross-doc parent link is constructible.

## 6. Org write-back — the brief's premise is inverted

The layout **is** written to org files, and that is correct, not a defect. The
projection puts both docs into ONE SQL store (`block_raw`), and org write-back
reads SQL, not Loro docs. Observed live in `verify-twoinst-1.log:521`:
`FileSyncController … doc=block:__default__ … held=4 authority=24`. "Device-local"
here means *not replicated to peers*, not *not persisted to disk*. Nothing in the
diff excludes the layout doc from write-back, and nothing should. No defect.

## 7. archlint stale entry — CONFIRMED and identified

`just analyze-arch` reports `baseline stale - 1 entry(ies) no longer fire`. I
identified it by copying `archlint/baseline.txt`, running `--update-baseline`,
diffing, then restoring by `cp` (sha256 `17ecc372f8403f50b0015fd2d36e0c6fbf99492ab1e7abf39f4c9ed9ed074fc7`
verified identical before and after):

```
fallback  crates/holon-loro/src/loro_document_store.rs
          /// Peer id to mint the global doc under. `None` = the env/random fallback
```

It is a `fallback`-rule hit on a **doc comment**, and it went stale because this
lane reworded that comment to "Peer id to mint both docs under. `None` = the
env/random default". So the stale entry **is this lane's**, count 110→109
(`fallback` 14→13). Leaving it for the weave is the right call — regenerating
would ratchet other live lanes' entries — but it should be named in the weave
note so nobody hunts for it.

## Findings for the orchestrator (no fixes applied)

1. **Coverage gap, medium.** Reverting `is_live_anywhere` at
   `block_cell_registry.rs:366` leaves the whole suite green. The lane's own
   "real defect fixed" claim has no regression guard. Recommend a test that boots
   an already-migrated store twice and asserts the layout is not re-minted.
2. **Coverage gap, medium.** `layout_migration.rs` has zero tests. My scratch
   tests passed and are a ready-made spec for three of them — the fail-loud arm
   in particular is boot-blocking, uncovered production code.
3. **Wording, low.** The report's "structurally outside `replicate_all`'s reach"
   is true of the code but is not what the green criterion pins; the criterion
   pins doc placement. Worth correcting so the next reader does not assume the
   `ContainerRegistry` property is test-guarded. It is not.
4. **Nit.** The lane report says the global snapshot is `holon.loro`; it is
   `holon_tree.loro`.
