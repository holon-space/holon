# E-solid shadow-mesh oracle IMPLEMENTED — adoption machinery deleted

**Session goal (from handoff `handoff-esolid-shadow-mesh-oracle.md`):** replace the two
check-time SUT-adoption functions in the composed PBT oracle with a shadow Loro peer mesh
that PREDICTS CRDT outcomes exactly; only scalar Lamport heights flow SUT→Ref.

## What landed (all uncommitted, this worktree)

1. **Spike s9** (`holon-loro/src/multi_peer.rs::clock_parity_spike`): the one un-de-risked
   mechanism — `ShadowDoc::clone` = `fork()` + `set_peer_id(original)` — proven: two nested
   mid-script clones reproduce a never-cloned run EXACTLY (incl. a post-clone peer-id
   tie-break that a fork-minted random peer id would scramble), and the fork is a deep copy
   (post-clone ops on the original don't leak). 9/9 spike tests green.

2. **`pbt/shadow_mesh.rs`** (new): `ShadowDoc` (LoroDoc + pinned peer id; Clone = deep fork
   + id restore; custom Debug) and `ShadowMesh` (primary peer 1 = PBT-pinned prod primary;
   peers `100+idx` = `LoroSut` scheme). Methods: `seeded_from_blocks` (one-shot seed — s8:
   base op shapes don't matter), lenient `pad_primary_to` (no-op at/below current height —
   generation-phase stale feeds), `fork_peer`, diff-driven `catch_up_primary`
   (membership/parent/content vs the ref block map), `sync_peer_bidirectional` (mirrors
   `apply_sync_with_peer`), `merge_peer_into_primary` (mirrors `apply_merge_from_peer`'s
   vv-delta import), peer ops via the same `peer_ops` helpers `LoroSut` drives, and the
   consume reads `primary_content` / `primary_children_order`.

3. **Clock side-channel:** `ReferenceState.clock_feed: Arc<Mutex<Option<u32>>>` (Clone
   shares — harness seam, like `IdResolver`). `composed/harness.rs::feed_sut_clock` writes
   `SutLoroLog::loro_lamport_height()` after `init_test` build and after every
   apply+settle. Ref-apply N therefore pads to the post-(N−1) height = exactly where SUT op
   N lands (walking skeleton #2's proven recipe).

4. **Centralized catch-up hook:** `declare_e2e_transitions!`'s `apply_to_ref` runs
   `state.shadow_catch_up_primary()` (pad → diff-mirror) after every variant dispatch —
   one site covers every machine (keystone wide, ReferenceMachine, slices); missed sites
   self-heal at the next catch-up.

5. **Mirror + consume:** `RefPeersMut::add_peer_from_primary_snapshot` lazily seeds the
   mesh, catch-up → pad → fork (spike boundary order); `peer_apply_{create,update,delete}`
   mirror into the shadow peer; `peer_apply_char_{insert,delete}` now mirror AND read the
   peer's block content back from the shadow (former documented no-op gap closed);
   `peer_sync_from_primary`/`peer_merge_into_primary` do catch-up → pad → shadow sync, then
   `merge_peer_blocks_into_primary` CONSUMES the shadow: merged content = shadow
   `read_text` (replaces `loro_merge_text` AND the LWW else-branch), created-group sibling
   order permuted to the shadow's converged child order (replaces
   `adopt_observed_peer_sibling_order`).

6. **Deleted:** `adopt_observed_peer_sibling_order`, `adopt_observed_crdt_merged_content`
   (+ their two default-`run_report` calls), `BlockState::crdt_merged_content` (field,
   ctor, `remapped_doc_uris` arm), `loro_merge_text`, `PeerRefState::baseline_contents` +
   `refresh_peer_baseline` (write-only once the merge consumed the shadow). Stale doc
   comments repointed (ui_harness, fixtures/mod, apply_mutation, merge_from_peer,
   structural_pbt).

7. **KEPT (per handoff):** `effective_sibling_sort_keys` prod fix, merge append-stamping
   (shadow order supersedes within-merge), all spike/skeleton/guard tests,
   `text_merge_provider.rs` untouched.

8. **Skill self-improvement:** `pbt-composition` anti-patterns gained "Adopting SUT
   observables into the oracle at check time" with the shadow-mesh pointer.

## Validation

- `holon-loro` suite 133/133 (incl. 9 clock-parity spikes with new s9).
- `holon-integration-tests --lib --features pbt` **160/160** (incl. both walking skeletons
  + `peer_merge_sibling_order_sql_matches_loro` guard).
- `holon-gpui --tests` / `holon-tui --tests` compile clean.
- Keystone `general_e2e_composed_pbt` (`sequential 1..40`, `PROPTEST_CASES=16`):
  - **Run 1 RED** — and it was a REAL gap the shadow design review missed: the first
    implementation permuted only blocks created **within one merge**. Minimal input
    (persisted as a cc seed, now green): 3×AddPeer at equal fork heights →
    `ApplyMutation(LoroPeer{2})::Create` under `parent` → peer 1 bumps its clock
    (`Update c1`) then creates under `parent` → `SyncWithPeer(1)` → `SyncWithPeer(2)`.
    The two creates are CONCURRENT (both pre-sync) so their fractional indices tie and
    Loro orders by op id — peer 2's create has the LOWER lamport and sorts FIRST even
    though it arrives via the LATER sync. Arrival order ≠ op-id order.
  - **Fix:** the shadow-order permute in `merge_peer_blocks_into_primary` now groups ALL
    `block:peer-…` siblings under each parent (across merges — the same grouping the old
    adoption used), ranked by `shadow.primary_children_order`. Earlier-merged peer blocks
    are already in the shadow primary, so cross-merge ranking is well-defined.
  - **Run 2 GREEN** (`1 passed`, 734s — ran concurrently with the lib suite, so wall time
    is not comparable to the 557s baseline). The run replays the persisted cc seed of the
    Run-1 failure first, so that exact shape is now a standing regression guard — the seed
    is KEPT deliberately (green, like the pre-existing one).
  - One lib test (`frontend_slice_displayed_text_viewmodel_bites_on_nested_content`)
    red-flaked while sharing cores with the keystone (viewmodel snapshot settle); passes
    in isolation and in the serial full run.

## Design notes / gotchas for future sessions

- **Lenient padding is deliberate:** `clock_feed` is Arc-shared across proptest cases
  (`Just(initial_state)` clones), so generation-phase ref evolutions can read a stale
  height; those states are discarded and execution re-evolves fresh. A pad target below
  the shadow height is therefore skipped, not a panic.
- **Boundary order matters:** fork/sync boundaries are catch-up → pad → fork/sync (fork
  height must equal the SUT's); mid-window primary edits are pad → mirror (op must land
  lamport-exact). The post-dispatch hook does pad → catch-up, which is the mid-window
  shape; the boundary shape lives inside the `RefPeersMut` methods.
- Shadow ids = the REF's bare stable ids (incl. `block::split-N` synthetics); Loro
  tie-breaks compare (lamport, peer id) only, so id spelling never matters, and the
  existing check-time resolver remaps as before.
- `merge_peer_blocks_into_primary` now fails LOUD (`panic: shadow mesh desynced`) if a
  merged/created block is missing from the shadow — a desync is a harness bug, never
  something to paper over.
