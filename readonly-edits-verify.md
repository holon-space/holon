# Verify `readonly-edits` — REFUTED

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/readonly-edits` (`pwd` confirmed),
`enforce_write_tier` present at `crates/holon/src/api/operation_dispatcher.rs:527,1110`.

## Gates (reproduced in this session)

| gate | log | result |
|---|---|---|
| `nextest -p holon-core -p holon-filesystem -p holon-app -p holon-architecture-tests -p holon-orgmode -p holon --no-fail-fast` | `lane-logs/verify-gate1-b.log` | 1163 run, 1158 passed, **5 failed = exactly the known `e2e_backend_engine_test` matview reds** ("cannot modify materialized view block") |
| `nextest -p holon-integration-tests --test cook_vault_ingest` | `lane-logs/verify-gate2.log` | 8/8 |
| `cargo check -p holon-gpui --tests` | same | clean |
| `just keystone-smoke` | same | 4 passed |
| `cargo fmt --check` | same | clean |
| `bugfunnel.py check` | same | 637 entries, 0 problems |

The lane's own gate numbers hold. The claim does not.

## REFUTED — the editor's keystroke path never reaches the dispatcher

`EditorViewModel::apply_local_edit` (`crates/holon-frontend/src/editor_view_model.rs:622-638`)
has a **cell-mode early return**: when a `Cell<String>` is attached and the write
is not a source-channel commit, it applies the diff straight into the CRDT via
`apply_local` → `LoroTextCellBacking::apply_text_op` → the block's real
`LoroText` in the vault doc, sets `self.buffer`, and `return Ok(None)` — **no
`OperationIntent`, no `execute_operation`, no `enforce_write_tier`**.

The cell is the real vault container: `BlockCellRegistry::live_field_any`
(`crates/holon-loro/src/block_cell_registry.rs:217-229`) resolves
`resolve_loro_text_container` on the live doc. GPUI attaches it unconditionally
when the registry resolves (`frontends/gpui/src/views/editor_view.rs:163-165`);
TUI does the same (`frontends/tui/src/app_main.rs:1013,1147`).

### Reproduced counterexample

Scratch probe appended to `crates/holon-integration-tests/tests/cook_vault_ingest.rs`
(same fixture as the lane's own refusal test: `Pancakes.cook` + `Notes.org`),
which resolves the prod `BlockCellRegistry`, takes the live `content` cell for
`block:Pancakes.cook::b::0` and applies one `TextOp::Insert`:

```
SCRATCH before="Crack the eggs into a bowl."
        authoritative_after=Some("HACKED Crack the eggs into a bowl.")
```
(`lane-logs/verify-bypass2.log:161`) — read back with
`BlockReader::get_block_authoritative`, i.e. the **write-authority Loro tree**,
the same oracle the lane's own test uses.

Expected (per the claim): refusal, unchanged authoritative content, a
`ShareDegradedReason::EditRefusedReadOnlyFormat` on the bus.
Actual: the op is minted in `.loro/holon_tree.loro`, silently, with no banner.
It survives a wiped SQL projection and replays on every boot — precisely the
condition entry `refused-write-back-still-persists-in-the-vault-loro-doc`
claims to have closed.

Scratch code removed; file restored by `cp` + sha256 match
`e44cda37abc0b6096496d06437f79d13b02c122cdad30bb9a66f6721e0ff4444`.

### What this falsifies, precisely

1. "A block whose owning document is homed in a `WriteTier::ReadOnly` file
   cannot be written from the store" — it can, through the cell.
2. "The decision is made once, at the ONE writer" — the dispatcher is not the
   one writer. The Loro text cell is a second, ungated writer.
3. Deferred item 1's wording ("typing now bounces off the dispatcher and raises
   a banner instead of forking the store, but the caret still lands") is wrong
   in prod cell mode: typing forks the store and raises nothing. The only thing
   the dispatcher gate stops is the blur/`set_field` commit — which, in cell
   mode, never fires for content anyway.
4. Entry 2's "no CRDT op is minted" is asserted only against the dispatcher
   `set_field` path (which is the correct doc-level oracle — see below), not
   against the path the user actually types on.

## Answers to the other probes

5. **Doc-level vs API-level (entry 2 part 1).** The lane's assertion IS at the
   write-authority-doc level (`get_block_authoritative` on the Loro tree), not
   only the store API. That part is well-built; it is simply scoped to a path
   the user does not take.
6. **`OpOrigin::Sync` pass-through.** Confirmed by code
   (`operation_dispatcher.rs:536`): a peer on an older build that made a
   store-origin edit into a read-only-homed doc syncs it in here unrefused. Per
   the stated rationale that is intended (refusing a merged peer history breaks
   the replica), and the lane's own deferred item 3 names the missing
   store-vs-disk invariant that would surface it. Reported, no remedy — but note
   the local bypass above produces exactly such a forked op, so it will
   propagate to peers.
7. **Empty-registry cost.** `ReadOnlyFormatGate::refusal_for` short-circuits on
   `ReadOnlyDocuments::is_empty()` before any store read, so an org-only vault
   never resolves a `BlockReader`. It is an uncontended `RwLock` read, taken up
   to twice per op (`id`, then `parent_id`), not "one atomic read" literally, but
   there is no per-op store I/O and no per-op exclusive lock. Acceptable.

## Not run

Teeth checks (uninstall the gate call / the bus raise and watch the two tests go
red) were not executed: the verdict is already REFUTED by a reproducible
counterexample, and the lane's `lane-logs/red-1.log` already records the
red-for-the-right-reason for exactly those two sites.
