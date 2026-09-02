---
id: 2026-09-02-structural-edits-in-a-shared-subtree-never-reach-the-peer
date: 2026-09-02
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Creating or deleting a block inside a shared subtree writes to the global Loro
  doc instead of the shared one, so the change never reaches the peer and the
  two devices diverge permanently with no error, no banner and no log line.
---

## Bug

Found by the `double-dogfood` lane on 2026-09-02, driving two live instances at
once: the GPUI desktop app on macOS and the GPUI Android app on an emulator
(`Medium_Phone_API_36.0`), paired over a real Iroh share.

Text and property edits replicate correctly in both directions. Structural
edits do not, in either direction, and nothing says so.

Measured sequence, page `block:dd-trip` shared from the Mac and accepted on
Android:

| Round | Operation | Side | Reached the peer? |
|---|---|---|---|
| R1 | `set_field` content | Mac | yes, under 4 s |
| R2 | `set_field` content | Android | yes, under 4 s |
| R3 | `create` child `dd-trip-4` | Android | **no** |
| R5 | `create` child `dd-trip-5` | Mac | **no** |
| R6 | `delete_subtree` `dd-trip-1` | Mac | **no** |
| R7 | `set_state` TODO | Mac | yes |
| R8 | concurrent `set_field`, same block, both sides | both | yes, converged identically |

R3 and R5 are the same defect mirrored, so this is not an Android problem. The
missing block was still absent after a further 20 s, and after a later edit that
demonstrably did cross the link in the same direction, so it is a permanent
divergence rather than a slow one. R6 shows deletes are lost the same way: the
block deleted on the Mac is still present on Android.

R8 is worth recording as a pass: two concurrent edits to the same block
converged to byte-identical content on both peers
(`CONCURRENTCONCURRENT from ANDROIDMAC`). Character interleaving is expected
from a text CRDT with no conflict UI; convergence is the property that matters
and it held.

**This is the "silently degrades to look fine" case the repo forbids.** A user
shares a page, adds a line on their phone, and the line simply is not on their
laptop. Neither app logs anything: `grep -iE "ERROR|PANIC|degraded|refus"`
over the Mac's `app.log` and the Android `logcat` shows no entry for any of the
lost writes.

## Root cause

The write path is mount-aware for text but not for tree structure.

`crates/holon-loro/src/loro_block_operations.rs:106-109`:

```rust
/// Find the backend containing a block (always the global backend).
async fn find_doc_for_block(&self, _: &str) -> Result<(String, LoroBackend)> {
```

The block id is discarded. Every operation resolves the global backend; the
per-block routing into a shared doc happens further down, inside `LoroBackend`,
which is why `set_field` and `insert_text` reach the shared doc.

`create` never gets that far. At `loro_block_operations.rs:747-748` it hardcodes
the global doc:

```rust
// All blocks live in the single global tree
let doc_id = String::new();
```

That comment stopped being true when subtree sharing landed. `create` has
`parent_id` in hand, which is exactly the key that would identify the owning
shared doc, and does not consult the mount registry (`shared_trees`, wired in at
lines 67 and 99).

Confirmed against the on-disk snapshots. After the rounds above, on the Mac:

| Content | in `shares/<id>.loro` | in `holon_tree.loro` |
|---|---|---|
| `CONCURRENT…` (a text edit) | yes | — |
| `item 5 born on mac` (a create) | no | yes |

The created node went to the global doc, which is never synced to the peer, and
the shared doc it belonged in never saw it.

This is documented blocker **B3** in `docs/Reference/SUBTREE_SHARING.md`, which
that document still describes as unfixed in full. It is now *partly* fixed —
text and property writes were made mount-aware — and the remaining half is
invisible precisely because the fixed half makes sharing look like it works.

## Missing piece

Two gaps, and the second is why the first survived.

**ENVIRONMENT.** No test drives two real peers. The keystone PBT runs one
instance, so a write that lands in the wrong local document is indistinguishable
from a correct one: the single SUT reads back exactly what it wrote. Only a
second peer can observe that the op never left.

**ORACLE.** There is no cross-peer convergence invariant. Even where the harness
does exercise sharing, nothing asserts that after settling, peer A's shared
subtree equals peer B's. Such an invariant would have gone red on R3 and R6.
Comparing raw rows will not work — each peer mints its own mount-block id, so
the oracle must compare the shared subtree by `(child id, content, properties)`
and ignore the mount id, as this lane's `scripts-lane/round.sh` does.

## Remedy

**Fixed 2026-09-02** by the `share-create-routing` lane. The mechanism was
narrower than the root-cause section above says, and the correction matters for
anyone reading this later.

`create` was ALREADY routed by its parent —
`create_under_shared_parent_lands_in_shared_doc` passed on the unfixed base.
The hole was the **mount node as the parent**. After a share, the page the UI
navigates to IS the mount (this entry's own "the sharer's page loses its stable
id" observation), so every create a user drives on a shared page carries the
mount's id as `parent_id`, and the mount is alive in the global tree — so the
parent resolved to the global doc and the child was born there. Same for the
accepter, whose page is likewise a mount.

The fix, in `crates/holon-loro/src/loro_backend.rs`: one resolution
(`ParentRoute`) returns the doc a child lands in PLUS the parent to resolve
inside that doc, and `route_through_mount` maps a mount parent to its share's
doc with the shared root as the effective parent. A mount whose share document
is not loaded, or whose mount metadata does not parse, is a loud `Err` naming
block and share — never a global write. Delete and move needed the read side
routed too (`list_children` for a parent inside a share, and for the mount
itself, plus the shared root's parent uri, which was panicking `get_meta`).

Both gaps this entry names are closed:

- **ORACLE** — P-STRUCT in `crates/holon/tests/sync_suite/sync_pbt.rs`: every
  child of the shared subtree a peer must see is alive in ITS shared doc, every
  deleted one is gone, every moved one hangs under its new parent. It went red
  for exactly this reason before the fix ("P-STRUCT/A: child block:s1 missing
  from shared doc; alive: [Child 1, Child 2, Shared heading]") on both peers.
- **ENVIRONMENT** — the share PBT now drives structural ops on BOTH peers
  through the production intent boundary (`execute_operation` with `create` /
  `delete_subtree`) against two live backends, so a write that lands in the
  wrong local document is observable.

**A separate engine defect found alongside this bug, now fixed:** on a shallow
share, a structural write used to panic the pinned loro fork (decision D70) when
it merged against any concurrent op on the other peer. Upstream loro `ddc47ecc`
fixes it, and Holon picked it up by rebasing the fork and bumping the pin; the
reproducer is now the convergence assertion
`structure_merged_against_a_concurrent_op_converges_on_a_shallow_share`.

## Adjacent observations from the same run, not filed separately

- **The shared page's org file on disk is empty.** After the share,
  `vault/__default__/Trip planning.org` is 0 bytes while the content lives in
  the shared Loro doc. Whether a mounted share is supposed to write back to org
  at all is a design question, but a 0-byte file that a user can see and open is
  a poor answer either way.
- **The sharer's page loses its stable id.** `block:dd-trip` no longer exists in
  SQL after sharing; it is replaced by a freshly minted mount block. The
  document alias still points `block:dd-trip` at the org file, so the alias
  outlives the id. Any `[[link]]` to a page would dangle the moment it is
  shared.
- **`unshare` and `gc_orphans` both exist** as `tree` operations.
  `SUBTREE_SHARING.md` states no unshare path exists; that is stale.
- **The ticket is schema v2** with a `capability` token and `expires_at`. The
  same document says the ticket-v2 authorisation handshake was deliberately
  deferred; that is also stale.
