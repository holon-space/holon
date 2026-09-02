# Verification — lane `pair-reimport` (D78.d)

**Verdict: REFUTED.**

The functional claim holds — the happy path does what the lane report says, and
every gate is green. The claim that fails is the safety one implied by the
design: the step is destructive with **no crash-safety and no recovery**. A
single mis-paste, a stale invite, or a second tap of "pair" moves the device's
only copy of its data into `<store>/archive/<ts>/`, empties the tree, and leaves
a store that boots EMPTY and SILENT on the next start, with nothing in the error
telling the user where their data went.

Workspace `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/pair-reimport`,
uncommitted diff on `main` 4e2ee368, 7 files. Scratch tests were added to a copy
of the test binary and deleted; `jj status` is byte-identical to the lane's.

## Gates — all green (reproduced)

| Gate | Result | Log |
|---|---|---|
| `cargo nextest -p holon-loro -p holon-sharing -p holon-app` | **591 run, 591 passed, 4 skipped** | `verify-logs/units.log` |
| `two_instance_composed_pbt` (once) | **24 run, 24 passed, 4 skipped** | `verify-logs/two_instance.log` |
| `just keystone-smoke` | **4 passed, 0 failed** | `verify-logs/keystone.log` |

(The lane's 746 counts five crates; my three-crate subset is the same suite,
consistent.)

Note: the workspace's PATH-default `cargo` is Homebrew stable and cannot build
this repo (`-Z threads` → "only accepted on nightly"). The rustup proxy
`/opt/homebrew/opt/rustup/bin/cargo` resolves `nightly-2026-08-16` correctly.

## Security findings

### HIGH-1 — Untrusted invite content destroys the store before it is validated

`pair_with_owner` (`crates/holon-loro/src/device_pairing_op.rs:797-826`) archives
and wipes at lines 798-800, and only at line 820 checks that the invite's
containers exist on this device. The invite is bearer data the user pastes; a
stale one, one from another vault, or one from a build with a different container
naming is enough.

Executed (scratch test, deleted): valid invite, `shared_tree_id` renamed:

```
[before] receiver live blocks = 34 ; holon_tree.loro exists=true ; archive_stamps=0
[error]  the invite advertises container `holon_tree-from-another-vault`,
         which this device has no document for
[after]  solo_note_present = false ; holon_tree.loro exists=false ; archive_stamps=1
[after]  error mentions the archive path: false
[next boot] live tree nodes on disk-backed store = 0
```

The last line is the decisive one: I re-opened the same store directory with a
fresh `LoroDocumentStore` — `get_doc` finds no `holon_tree.loro`, logs
`Creating new global LoroTree document`
(`crates/holon-loro/src/loro_document_store.rs:179-189`) and boots an **empty
store, silently**. `grep` over `crates` and `frontends` finds no code that reads
`archive/` — it is a hand-recovery-only backup nothing points at.

**Minimal fix.** Move every check that can refuse (`replication_set` container
lookup, endpoint creation, and ideally a reachability probe of each ticket) ahead
of `own_content`/`archive_documents`/`wipe_global_tree`. Then failure leaves the
device untouched, as the code's own comment at line 787 already promises for the
mounts check.

### HIGH-2 — Pairing twice destroys the store; no idempotence guard

No "already paired" state exists and `pair_with_owner` is unconditional. Executed
with the SAME valid invite twice:

```
[pair 1] Ok(())        archive stamps = 1
[pair 2] Err(... pairing dial of container `holon_tree`)   <- roster already at
         capacity (1 peer): "enrollment rejected ... enrollment refused"
[pair 2] archive stamps = 2 ; solo_note_present = false ;
         holon_tree.loro exists=false ; holon_layout.loro exists=false
```

The second pair archived and wiped again, and only then was refused by the
owner's roster. The device is now in the HIGH-1 state after an entirely ordinary
user error, and recovery requires the user to pick the RIGHT one of two timestamp
directories by hand.

**Minimal fix.** Record that this device is paired (a marker in the store, or
simply the presence of `archive/`) and refuse a second `pair_with_owner` loudly
before any capture.

### HIGH-3 — Every kill point between archive and the final `save_all` boots empty

Nothing is persisted between `archive_documents` (line 798) and `save_all` (line
857). `get_doc` caches the document in memory, so the wipe, the adoption and the
re-import all live in RAM until that last save. Therefore a kill between archive
and wipe, between wipe and dial, or between dial and re-import all leave the same
on-disk state that HIGH-1 evidences: no `*.loro` in the store dir, the user's data
in `archive/<ts>/`, and a silent empty boot. A kill MID re-import is worse:
`LoroBlockOperations` calls `store.save_all()` on each write
(`crates/holon-loro/src/loro_block_operations.rs:120`), so a partially adopted +
partially re-imported store can be persisted and boots silently as a partial
vault.

**Minimal fix.** Write a `pairing-in-progress` marker (naming the archive
directory) before the archive, delete it after the final `save_all`, and make
boot refuse to open an empty/partial store while that marker exists — surfacing
the archive path instead of a blank vault.

### HIGH-4 — Re-pairing to a DIFFERENT owner exfiltrates the first owner's vault

`own_content` (line 433) is "every live block that is not app-seeded". After a
successful pair, the OWNER's entire vault satisfies that. So pairing the device to
a second owner captures owner A's content and re-imports it into owner B's store,
where it replicates to B. No guard, no disclosure beyond a block count. The fix
for HIGH-2 closes this as well.

### MEDIUM-1 — A block whose id the adopted store already holds is dropped silently

`reimport` line 604: `if adopted.contains(id) || placed.contains(..) { continue; }`
— no content comparison, no count, no name. This is exactly the brief's case (3)
(a phone that already holds owner uuids and edited them locally): the phone's
version of every colliding id is discarded, only its children are re-homed. The
one-node-per-id postcondition still passes, because the node was never created,
so the invariant is satisfied while content is lost. The informational event
reports `requests.len()` only, so nothing tells the user which blocks lost.

**Minimal fix.** Count and disclose skipped ids alongside `blocks`; refuse (or
warn loudly) when a skipped id's captured content differs from the adopted one.

### MEDIUM-2 — A failed re-import leaves the device silently paired

`reimport` orphan refusal (`ReimportHasNoParent`, line 624) and
`assert_one_node_per_id` (line 855) both run AFTER the dial. On either, the
operation reports failure, but the adopted store is already in memory and the
next block write's `save_all` persists it: the user sees "pairing failed" on a
device that is in fact paired and has lost its own content to the archive. The
orphan path is reachable — a block hanging under a seeded id the owner lacks (a
different bundled `index.org` between app versions gives different
`bundled_layout_ids`).

### LOW-1 — Archive path is disclosed in the event, contrary to the brief

Brief item (5) asked for counts only. `ShareDegradedReason::PairingReimportedLocalContent`
carries `archive: String` and the GPUI toast renders it
(`frontends/gpui/src/share_ui.rs:337-352`). It is a local path under the user's own
store dir, not key material, and it is the ONLY recovery affordance that exists —
I judge the disclosure justified, but it is a deviation worth a ruling. No
`tracing` call exists in `device_pairing_op.rs` at all, and the invite string is
never logged or embedded in an error; `invite_fingerprint` is the only naming path.

## Checked and clean

- **Archive rename failure (read-only dir).** `create_dir_all` (line 496) and the
  first `rename` both run BEFORE `wipe_global_tree` (line 799) and `bail`. The
  device is left intact; this is the one failure mode that is safe.
- **Zero-archived guard** (line 514) is real and errors before the wipe.
- **Re-import fidelity.** `BlockCreateRequest::of`
  (`crates/holon-core/src/block_ordering.rs:53-68`) carries id verbatim, content,
  the full properties map plus `collapsed`/`widget_only`, and `BlockEdges::of`
  (internal links, tags). Sibling order is the capture's `(parent, sort_key, id)`
  sort replayed as batch order — phone content lands after the owner's under a
  shared parent, which is sane. `sql_projection_lag` is empty on the happy path
  and the day-block union oracle passes (`two_instance` 24/24).
- **`assert_one_node_per_id`** counts `build_tid_index` values, which is keyed by
  `TreeID` over LIVE nodes only (`loro_backend.rs:1775-1800`) — the duplicate count
  is sound; the lane report's "silently overwrites" caveat concerns the read path,
  not this check.
- **Wipe scope.** Only the global doc is wiped; the device-local layout document is
  archived but not modified, and the final `save_all` rewrites it from memory.

## Needs a human security ruling

1. Should the destructive steps be gated behind a reachability handshake with the
   owner (HIGH-1/HIGH-3), i.e. dial first into a scratch document and only then
   archive+wipe+import?
2. Is a second `pair_with_owner` a refusal or a supported re-pair (HIGH-2/HIGH-4)?
3. Is the archive path in the user-visible event acceptable (LOW-1)?
