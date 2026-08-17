---
id: 2026-08-09-refused-block-create-leaves-sibling-key
date: 2026-08-09
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A REFUSED block create leaves its sibling `sort_key` re-key durably written
  — a keyspace rewrite performed for a block that does not exist.
source_line: 1192
---

## Bug

(ADR 0030 second review, reading the SqlOnly order-owner code; no automated
test covered it) **A REFUSED block create leaves its sibling `sort_key`
re-key durably written — a keyspace rewrite performed for a block that does
not exist.** `SqlBlockOperations::create`
(`crates/holon/src/core/sql_block_operations.rs`) minted the position first,
and minting over a sibling set that is not an insertable sequence (a tie, or
a row still on the `"A0"` column default) rewrote every displaced sibling's
`sort_key` through its own `set_field` — its own transaction — BEFORE the
create INSERT. Reachable with no crash: the ADR 0029 D1b derived-id
collision refuses the create afterwards (a page create whose deterministic
id is held by a since-renamed page). Red proof
`a_refused_create_leaves_no_sibling_rekey_behind`: the sibling read back
`[("block:unkeyed","7F80")]` where the untouched keyspace says
`[("block:unkeyed","A0")]`. The re-key is order-preserving, so the no-crash
damage is unattributed durable writes — no `OperationResult`, hence no
inverse and no undo, plus CDC and org write-back churn for an operation
reported as failed; the crash variant MIS-ORDERS the parent (ADR 0030
validation V3).

## Root cause

task #25 lane, found by the ADR 0030 second review reading the SqlOnly
order-owner code (outside any automated test): **a REFUSED block create
leaves its sibling re-key durably written — the store keeps a keyspace
rewrite performed on behalf of a block that does not exist.** Reachable with
no crash and no fault injection: `SqlBlockOperations::create`
(`crates/holon/src/core/sql_block_operations.rs`) minted the new block's
position first, and minting over a sibling set that is not an insertable
sequence (a tie, or a row still carrying the `"A0"` column default) rewrote
every displaced sibling's `sort_key` through its own `set_field` — i.e. its
own transaction — BEFORE the create INSERT. Any later refusal keeps them:
the ADR 0029 D1b derived-id collision (`IdentityCollision`,
`sql_operation_provider.rs` create arm) is the ordinary path, e.g. a page
create whose deterministic id is held by a since-renamed page. Red proof,
`a_refused_create_leaves_no_sibling_rekey_behind`: after the refusal the
sibling read back `[("block:unkeyed", "7F80")]` where an untouched keyspace
says `[("block:unkeyed", "A0")]`. The re-key is order-PRESERVING, so the
no-crash damage is not visible wrong order but unattributed durable writes:
they belong to no `OperationResult`, so they carry no inverse and undo
cannot restore them, and they churn CDC and org write-back for an operation
the user was told failed. The crash variant is worse — ADR 0030 validation
V3 (below, two tests) shows a partially applied re-key MIS-ORDERS the parent
for keyspaces sitting below the generator's output, one write being enough.
COVERAGE: the keystone generates no REFUSED create at all — every create in
the catalog is built to succeed — so no transition sequence reaches the
state where post-refusal residue could be observed. Secondary ORACLE: even
if it did, nothing would judge it — `sort_key` is internal (ADR 0005, absent
from the domain `Block`), so `inv-blocks-match-ref` compares ORDER, which an
order-preserving re-key leaves identical. FIXED in-lane by making the mint
pure: `OrderKeyMinting::new_child_anchor` returns a `MintedPosition` (key +
the sibling re-keys it is expressed against) and the re-keys ride the op
into its OWN transaction (`_order_rekeys` control param →
`SqlOperationProvider::order_rekey_statements`);
`MintedPosition::into_params` takes `self` by value so a caller cannot spend
the key and drop the re-keys. GAP NOT CLOSED, filed as follow-up: the
keystone still cannot generate a refused create, and still has no "a refused
op leaves no durable delta" invariant — the fix is pinned by unit-level
regressions in `sql_block_operations.rs`, not by the keystone. Sibling
defect fixed in the same lane but NOT counted here (crash/DB-error window
only, not reachable by ordinary refusal): SqlOnly `place()` split one
placement into a `parent_id` transaction and a `sort_key` autocommit, so a
failure between them left the block re-parented under its old parent's key —
now one `place_row` transaction, red-proven by
`a_refused_placement_leaves_neither_half_of_the_move_behind` with a
storage-seam fault)

## Missing piece

the keystone generates no REFUSED create at all (every create in the catalog
is built to succeed), so the post-refusal state is unreachable; and
`sort_key` is internal (ADR 0005), so `inv-blocks-match-ref` judges ORDER,
which an order-preserving re-key leaves identical

## Remedy

FIXED 2026-08-09 (task #25, ADR 0030 D1 remediation): the mint is now pure —
`OrderKeyMinting::new_child_anchor` returns a `MintedPosition` (key + the
sibling re-keys it is expressed against) and the re-keys ride the op into
its OWN transaction via the `_order_rekeys` control param
(`SqlOperationProvider::order_rekey_statements`, `place_row`);
`MintedPosition::into_params` takes `self` by value so a caller cannot spend
the key and drop the re-keys. GAP NOT CLOSED: the keystone still cannot
generate a refused create and still has no "a refused op leaves no durable
delta" invariant — the fix is pinned by unit regressions in
`sql_block_operations.rs`, not by the keystone.
