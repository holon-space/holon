# D78 rev 2 — pairing a phone that was already used standalone

Martin's objection to rev 1: "an irreversible decision (pair or start solo) at
the first open on a phone is a no-go." This document restarts the question
from that requirement.

## 1. What the problem concretely is

Both devices run the same boot code. That code creates ("seeds") blocks with
FIXED ids, for example `block:journals` (the parent of all journal day
blocks) and the bundled layout under `block:__default__`. A Loro tree node is
identified by the CRDT as (peer, counter), not by our stable id. So when two
devices seed `block:journals` independently and then pair, the merged tree
holds TWO live nodes that both claim the stable id `block:journals`. Every
reader keyed by stable id collapses them into one entry, so the duplicate is
invisible until a child is put under the "wrong" one.

Measured (lane pair-inc2, report
`.claude/worktrees/pair-inc2/lane-report-pair-inc2.md` §1): after one
whole-vault round, 31 ids are doubled. 25 are bundled layout (solved by D77.b:
the layout moves to a device-local doc that never replicates). 5 are journals
machinery. 1 is the journal day block for "today", which BOTH devices mint
under a date-derived id.

D77.b removes the 25. This decision is about the remaining 6 and, more
generally, about ANY content a phone created before it was paired.

## 2. The first-principles requirement (rev 2)

1. A user may install Holon on a phone, use it for a week, and pair it later.
   Nothing they wrote may be lost, and no dialog at first open may ask them
   to predict this.
2. After pairing, every fixed id names exactly ONE live node on both devices.
3. Pairing must not write DELETE operations into the shared CRDT on the
   strength of a guess about user intent.
4. Whatever pairing does must be reversible for a bounded time (the
   pre-pair state stays on disk as an archive).

Requirement 1 is what rev 1 violated: J-a (bootstrap before seed, refuse a
seeded receiver, D74.a) makes "used it, then paired it" a reset.

## 3. Options

### J-d — pairing = adopt the owner's store, then RE-IMPORT the phone's own content (recommended)

What it is. Pairing on a non-empty receiver does NOT merge two CRDT histories.
It does three steps, all on the receiver, before the receiver joins
replication:

1. Move the receiver's current store aside (`holon.loro` → an archive
   directory with a timestamp). Nothing is deleted.
2. Bootstrap the receiver from the owner's snapshot (this is the existing
   D74.a path: an empty store filled from the owner). Now the receiver holds
   exactly one node per fixed id, the owner's.
3. Walk the archived store and re-import the phone's USER content as ordinary
   new operations through the typed-rows ingest seam (the same path org files
   and external peers use). Fixed-id nodes in the archive are NOT re-created;
   their children are re-parented under the owner's node with the same fixed
   id. A day block whose date already exists on the owner's side has its
   children appended under the owner's day block. User blocks keep their uuids
   (they cannot collide; only fixed ids are shared by construction).

Example. Phone used solo for a week: 6 journal days, 40 blocks, one page
"Ideas". Owner vault: 900 pages. Pairing: the phone's store is archived; the
owner's 900 pages arrive; the 6 days are appended under the owner's
`block:journals` (2 of them merge into days the owner also wrote on); "Ideas"
arrives as a new page with its uuid intact. Links between the phone's own
blocks still resolve. The shared CRDT never saw a duplicate `block:journals`.

Decisive tradeoff. The phone's pre-pair EDIT HISTORY is flattened to a
snapshot (the re-import creates the content anew), so undo across the pairing
boundary is gone. In exchange the shared history never contains a duplicate,
no CRDT delete is written, and the archive makes the step reversible.

- Pro: satisfies all four requirements; no delete from inference; reuses the
  generic ingest seam (this is exactly the "generic reusable Rust" direction
  of the low-code program, not a pairing special case).
- Pro: D74.a ("empty receiver required") becomes a non-issue: a non-empty
  receiver is imported, not refused. D73.a (refuse when the receiver has
  MOUNTS) stays.
- Con: history flattening (above). Con: the re-import needs a rule per
  fixed-id family ("children go under the owner's node"), which is a small
  table today (journals machinery + day blocks) and must be kept when new
  fixed ids are added — boot assertion 2 (one node per fixed id) catches a
  forgotten row loudly.

### J-b′ — merge the CRDT histories, then run a deterministic fixed-id repair

What it is. Pair as today (both histories merge). Afterwards a repair pass
finds every fixed id with two live nodes, keeps the node created by the lower
peer id, moves the other's children under it, and deletes the loser. Both
devices compute the same result from the converged state.

- Pro: keeps the phone's full edit history in the shared CRDT; nothing is
  re-created.
- Con: writes DELETEs into the shared CRDT based on a rule, violates
  requirement 3 in letter (though for FIXED ids the "guess" is weak: the app
  itself defines these nodes as singletons, so deleting a twin is enforcing
  an invariant, not guessing intent). Con: the repair must run on BOTH sides
  and be idempotent under concurrent edits to the twins; needs its own PBT.
  Con: the duplicate exists in the shared history forever (only tombstoned).

### J-e — lazy seeding (reduce the window; not sufficient alone)

Do not seed `block:journals` or a day block until the user's first write.
A phone that is installed and paired before any write never mints anything.

- Pro: trivial; shrinks the problem to "used before pairing".
- Con: does not solve "used before pairing". Only useful as a rider on J-d
  or J-b′.

### J-f — deterministic op ids for fixed-id seeds (rejected, recorded for completeness)

Seed fixed-id nodes under a reserved peer id with fixed counters, so both
devices produce the SAME Loro op and the CRDT deduplicates it.

- Con: two docs generating DIFFERENT ops under one peer id is undefined
  behaviour in Loro; any drift in the seed (a layout yaml change, a version
  skew) corrupts the history. Not acceptable.

## 4. Recommendation and what it rests on

J-d, with J-e as a rider. It rests on three facts:

- The ingest seam already exists and is generic
  (`crates/holon-core/src/file_format.rs`, `TypedRowSet`,
  `DispatchingTypedRowSink`); re-import is a new SOURCE for it, not new
  machinery.
- Seed idempotence already exists (`BlockCellRegistry::create_entity` skips
  when the id resolves; report §4.1), so step 2 followed by a normal boot
  mints nothing.
- Fixed ids are a closed, app-owned list, so "children under the owner's
  node" is a table, not an inference.

The UX consequence: no question at first open. At pairing time the phone
shows "N notes written on this phone will be added to the shared vault"
(informational, with an archive kept). That is the only place a message
appears, and it is not a decision.

## 5. What J-d changes elsewhere

- D74.a: "pairing requires an empty receiver" is replaced by "pairing
  archives and re-imports a non-empty receiver". The loud refusal stays only
  for mounts (D73.a).
- The two-writer PBT harness gains a scenario: receiver boots solo, writes,
  pairs; oracle = union of content, one node per fixed id, phone uuids
  preserved.
- Boot assertion: every fixed id resolves to exactly one live node; fail
  loud (kept from rev 1).
