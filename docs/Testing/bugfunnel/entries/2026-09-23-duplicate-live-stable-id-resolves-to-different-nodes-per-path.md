---
id: 2026-09-23-duplicate-live-stable-id-resolves-to-different-nodes-per-path
date: 2026-09-23
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  When a CRDT merge leaves two live Loro nodes with the same STABLE_ID, the
  projection snapshot, the resolver scan and the warm cache path can each pick
  a different node for that id, and the shared stable-id cache keeps the first
  pick for the life of the process.
---

## Bug

Found by code audit during the write-authority lane (plan
`~/.claude/plans/decision-reads-write-authority.md`, Inc 0b review, workspace
`readauth-2`). No test or user saw it yet.

Two live nodes can carry one stable id. Two devices that each booted alone mint
their own node for every fixed id the app seeds (`block:journals`, today's day
block, the layout). A CRDT merge of those histories keeps both nodes alive.
`crates/holon-loro/src/device_pairing_op.rs:15-24` and `:760-775` state this
and avoid it for pairing only: the receiver wipes its tree before it adopts the
owner's history. Any other merge path (a share, a merge of concurrent creates of
one convergent id) has no such guard.

## Root cause

Each read path resolves a duplicate id by its own rule:

- The projection snapshot keeps the LAST live node in `get_nodes` order
  (`snapshot_blocks_from_doc_settled`, `blocks.insert` at
  `crates/holon-loro/src/loro_backend.rs:1519`). SQL projects that node.
- The resolver scan keeps the FIRST live node in `get_nodes` order
  (`find_tree_id_by_stable_id_sync`, `seen.entry(sid).or_insert` at
  `loro_backend.rs:4287`); `find_stable_id_in_doc` (`:1951`) also takes
  the first match.
- The warm path serves whatever node the cache holds: the node this process
  created or re-keyed (`:3919`, `:4373`, `:4392`) or the scan's pick.
  `cached_live_node` (`:1062`) checks that the cached node is alive and carries
  the id. It does not check that the node is the only one.

Since Inc 0b one `StableIdCache` serves every backend of a
`LoroBlockOperations`. So the first pick holds until that node dies. Writes by
id go to one node while SQL shows the other, and the answer depends on which
path ran first after boot.

## Missing piece

No keystone transition merges two histories that minted the same stable id.
No invariant asserts "one live node per stable id" on the Loro tree.

## Remedy

OPEN, not fixed in this lane. Proposal: one deterministic rule for every path —
the winner is the live node with the smallest `TreeID` (peer, then counter).
It is the same on every peer and in every process. The scan and the snapshot
already visit every live node, so both apply the rule and disclose each
duplicate with an `error!` that names the id and both `TreeID`s (a visible
fallback, not a silent one). A cached entry can name the loser after an
import, so an import that brings in a second live node for an id drops that id
from the cache. A keystone invariant "one live node per
stable id, or a disclosed duplicate" closes the oracle gap.
