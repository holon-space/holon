# The order of LogSeq's three datom indexes

Increment B (tree mutation) has to insert a datom in the position LogSeq would
have put it. That position is decided by three comparators. This file states
them, and states which parts are **measured** and which are only **read from
source** — the distinction matters, because this lane has twice been wrong
about LogSeq's runtime behaviour by reading rather than measuring.

Everything under "The rules" is measured. Sources are named for orientation
only; where a source reading and a measurement disagreed, the measurement won
and the reading is recorded as refuted.

Sources: `datascript/db.cljc` at the pinned fork rev
`3f141af97b70e1f14c65eaa119acd822ebece37e` (scratchpad `ds-fork`), reached from
`storage.cljs restore-by`. Measurements: LogSeq's own nbb runtime at oracle rev
`fab27740` against the committed fixture (2609 datoms) plus purpose-built
in-memory graphs.

## The sort keys

Each index is a B+-tree whose order is a lexicographic tuple, short-circuiting
on the first non-equal component (`combine-cmp`):

| index | sort key |
|---|---|
| eavt | `(e, a, v, tx)` |
| aevt | `(a, e, v, tx)` |
| avet | `(a, v, e, tx)` |

`avet` holds only attributes the schema marks `:db/index true` or
`:db/unique` — 2264 of the fixture's 2609 datoms.

Two properties of the fourth component matter:

- **`tx` compares by MAGNITUDE.** `datom-tx` is `(if (pos? tx) tx (- tx))`, so
  the assert/retract sign that the tail encodes is **not part of the sort key**.
  A retraction and its assertion sort identically on `tx`.
- Two datoms differing in no component cannot both exist, so the key is unique
  and the tuple is a strict total order (verified below).

## The rules

### Attributes: `(namespace, name)`, not the printed keyword

An attribute sorts by its namespace, then its name — **not** by its printed
form. The two differ whenever one namespace is a prefix of another, because
`/` (47) sorts after `-` (45) and `.` (46):

| pair | (ns, name) order | printed order |
|---|---|---|
| `:logseq.property/built-in?` vs `:logseq.property.class/extends` | built-in? first | extends first |
| `:b/z` vs `:b-a/a` | `:b/z` first | `:b-a/a` first |

MEASURED both ways round: on the fixture, the printed-form rule produces 4
violations in eavt and 1 in aevt while `(ns, name)` produces none; and an
in-memory graph carrying `:b/z`, `:b-a/a`, `:block/title`,
`:logseq.property/built-in?`, `:logseq.property.class/extends` reports them in
exactly `(ns, name)` order.

### Values: type group first, then within-group order

Values of different types never interleave. The type groups are totally
ordered:

```
boolean < inst < number < string < uuid < map/other < keyword < vector
```

MEASURED pairwise through the index itself — for every pair of the eight types,
a two-datom indexed attribute was built and the winner read back. The resulting
relation is a complete strict total order: each type sorts before exactly
7, 6, 5, 4, 3, 2, 1, 0 of the others, with no pair ordered both ways.

This is NOT a sort of type names, which is what `class-compare` looks like it
would do in source — "boolean" < "cljs.core/Keyword" < "number" would put
keywords second, and they sort second-to-LAST. The mechanism is unresolved
(nbb's SCI exposes neither `IComparable` nor real type names); the ORDER above
is measured and is what a writer must reproduce.

Within a group:

| type | order |
|---|---|
| boolean | `false` before `true` |
| number | numeric |
| string | code-unit (plain lexicographic) |
| uuid | by string form |
| keyword | `(namespace, name)`, exactly as for attributes |
| vector / seq | **count first**, then element-wise (`seq-compare`) — so `[1]`, `[2]`, `[1 2]` |
| map / other | by **ClojureScript's `hash`** |

### The hash-ordered tail of that table is the expensive one

A value that is neither a comparable native nor sequential — in practice a
**map**, e.g. `:logseq.property/icon`'s `{:type :tabler-icon, :id "table"}` —
falls to datascript's `:else` branch and is ordered by `(hash v)`,
ClojureScript's murmur3-derived hash.

To PLACE such a datom, Holon would have to reproduce ClojureScript's `hash`
for maps bit-for-bit, including its unordered-collection mixing. The fixture
carries 42 such datoms across 2 attributes.

### RULED: refuse rather than reproduce the hash

Holon **refuses to write any datom whose value is not one of the directly
comparable measured types** — a named error giving the attribute and the
value's shape. Reproducing the hash is deferred behind the first increment
that must write a map-valued property.

The refusal is on values HOLON WRITES. Existing map-valued datoms are read and
carried unchanged. That is sound only under a precise invariant, derived from
the order above rather than assumed:

> **A hash is needed only to order two map values AGAINST EACH OTHER.**

Because comparison is lexicographic, two values are compared only when every
earlier component is equal, and then:

- Holon's value vs an EXISTING map value → resolved by the **type-group rank**
  (map is 5, string 3, number 2 …). Different groups never need the hash.
- Holon's value vs another comparable value → resolved within its group.
- map vs map → needs the hash, and is the only case that does.

So no comparison Holon performs can require a hash, provided BOTH hold:

1. **Holon writes no map/other-valued datom** — asserts and retracts alike. A
   retraction carries its value and must be LOCATED in the tree, which is an
   ordering operation, so a map-valued retract is refused for the same reason
   as an assert.
2. **The writer only inserts and removes; it never re-sorts an existing set.**
   Incremental insertion compares the new datom against existing keys and
   never two existing keys against each other; a leaf's maximum (which a branch
   separator needs) is its last key, already ordered; a split is positional.
   A full REBUILD from an unordered datom set would have to order existing
   map values pairwise and is therefore forbidden while (1) stands.

Both are enforceable in code and both must be enforced: dropping either one
silently reintroduces the hash requirement.

#### Verifying a tree needs the hash; writing into one does not

An asymmetry worth stating, because the in-repo test ran into it. Checking
that an EXISTING index is fully ordered means comparing every adjacent pair,
including pairs that are both map-valued — so a full order verification cannot
be completed without the hash. Inserting a non-map datom never compares two
maps, so it can.

Measured on the fixture by `tests/tree_order.rs`, which compares every
adjacent pair with Holon's own comparator:

| index | pairs confirmed strictly increasing | undecidable without the hash |
|---|---|---|
| eavt | all | 0 |
| aevt | all | 0 |
| avet | all but 4 | 4 |

eavt and aevt reach a value comparison only once `(e,a)` / `(a,e)` already
tie, so they never need the hash at all. The 4 avet pairs are map-against-map
under `:logseq.property/icon`. The test asserts all three counts rather than
assuming them, so a change in what we cannot decide is a failure rather than
drift.

**Equality is decided before order**, which is why the count is 4 and not the
5 that a naive rank-then-order rule gives: two neighbours there hold the SAME
map, and equal values compare equal without any need to order them, letting
the comparator move on to the entity component. Datascript's `value-compare`
tests `(= x y)` as its very first clause for the same reason — and a branch
separator, which is a COPY of its subtree's maximum, is only comparable to
that maximum because of it. This decides equality only; two genuinely
different maps remain undecidable and are still refused.

## Order does not depend on insertion sequence

Checked because it decides whether tree shape is a function of the datom set at
all: had it not held, no writer could predict the tree without replaying every
transaction in order.

The same 19 values were transacted in four different sequences — declaration
order, reversed, rotated, and hash-sorted — and the resulting `avet` order was
identical every time. So the index is a pure function of its datom set.

## The root COLLAPSES when it is left with one child — measured

A writer that only ever splits would keep building deeper trees than LogSeq's
for exactly the same datoms, so this had to be settled by measurement rather
than left for a byte comparison to reveal.

A storage-backed graph was grown and then retracted down in stages, reading
addr 0's `shift` back each time (`shift` is depth − 1):

| datoms | eavt `[count shift]` | avet `[count shift]` |
|---|---|---|
| 1500 | `[1500 2]` | `[1500 2]` |
| 500 | `[500 1]` | `[500 1]` |
| 150 | `[150 1]` | `[150 1]` |
| 3 | `[3 0]` | `[3 0]` |

So the tree loses levels on the way down, one at a time. The rule Holon
implements is the narrow one the data supports: **after a removal, while the
root is a branch with exactly ONE child, the root becomes that child.** A root
branch with two children is legal — minimum occupancy does not apply to a root
— so nothing collapses there.

Encoded twice, on purpose: `EditableTree::remove` performs the collapse, and
`check_invariants` REFUSES a single-child root, so an implementation that
stopped collapsing fails rather than quietly deepening.

(Probe log: `scratchpad/logs/root-collapse.log`. The intermediate rows where
the count does not move are batches too small to overflow the tail, so no store
happened — the tail behaviour, not the tree's.)

## Redistribution picks the SMALLER sibling, ties going right

When a node falls to the minimum occupancy and neither neighbour is small
enough to absorb it, its keys are redistributed with a sibling — and the
sibling chosen is **the smaller of the two**, with a tie going RIGHT. Not the
left one by preference, which is what a partial reading of `rotate` suggests
and what cost this lane a day.

Measured by driving edit 11 of the head-to-head as two separate transactions
and storing after each half:

| step | leaves 1000335 / 1000336 / 1000337 |
|---|---|
| after the identical 10-edit prefix | 23 / 17 / 23 |
| after the RETRACTION alone | 23 / **19 / 20** |
| after the assertion | 23 / 19 / 20 |

The retraction takes 1000336 to 16. Both siblings hold 23, so neither can
absorb it; `23 < 23` is false, so it redistributes RIGHTWARD — 16 + 23 = 39,
split 19/20. A left-first rule gives 19/20 on the OTHER pair and is what Holon
did before this was measured.

The same rule explains the second divergence, where the neighbours differ:
left 18, right 17, so the right one is smaller and 16 + 17 → 16/17 — which
leaves the partition looking untouched, exactly what LogSeq produced while a
left-first rule visibly moved a datom.

The condition in `persistent-sorted-set`:

```clojure
;; left has fewer nodes, redestribute with it
(and left (or (nil? right) (< (node-len left) (node-len right))))
```

With this rule Holon and LogSeq produce **byte-identical files** for the same
17 edits — 458 of 458 rows, content and child pointers alike. That equality is
asserted by `head_to_head_with_logseqs_own_flush`, and
`bisect_the_partition_divergence` fails the moment the two disagree at any edit
count, so a regression in split, merge, redistribution or root collapse cannot
pass quietly.

## Unreferenced rows exist in the wild

The committed fixture's `kvs` table holds 456 rows: the head (addr 0), the
tail (addr 1), and 454 tree rows. Only **437** of those are reachable from the
three index roots. The other **17 are unreferenced** — nodes LogSeq merged away
and then left behind, because its storage layer discards its own delete list.

So a writer must not assume every row belongs to a tree, in either direction:
emitting fewer than the reachable nodes means a subtree went unserialized,
while treating every row as reachable would resurrect garbage. Pinned by
`serializing_an_unedited_tree_reproduces_its_rows_exactly`, which asserts the
count both ways.

This is also why W0's "456/456 byte-identical" is consistent with only 437
nodes being tree nodes: the kvs writer copies every row, orphans included,
rather than rebuilding the table from the trees.

## Verification against the real graph

The rules above, implemented independently in Python
(`scratchpad/check_order.py`) and applied to the fixture's datoms as dumped by
LogSeq itself (`script/dump_index_order.cljs`, run through the oracle):

| index | datoms | violations | keys distinct | re-sort reproduces LogSeq's order |
|---|---|---|---|---|
| eavt | 2609 | 0 | 2609/2609 | yes |
| aevt | 2609 | 0 | 2609/2609 | yes |
| avet | 2264 | 0 | 2264/2264 | yes |

Keys are distinct and the sequence is strictly increasing, so "no violations"
is the strong statement rather than the weak one: re-sorting the datom set by
these rules reproduces LogSeq's own sequence exactly, in all three indexes,
across 7482 positions.

The dumped counts also match addr 0's own metadata (`eavt-metadata` 2609,
`aevt-metadata` 2609, `avet-metadata` 2264), which is an independent check that
the dump read the same trees the writer will.

## What is NOT covered

- **Cross-type ordering is measured only for the eight types listed.** A value
  type outside them has no rule here and must not be written blind.
- The `nil` handling in the non-quick comparators (`cmp`/`value-cmp` return 0
  when either side is nil) is for seek/boundary datoms in range queries. Stored
  datoms never carry nil, and B must not emit one.
- Insertion and deletion use the `-quick` comparator variants
  (`db.cljc:1484-1494`) while restore uses the non-quick ones
  (`storage.cljs:155-157`). They agree on non-nil input, which is every stored
  datom — read from source, NOT measured, and worth a probe if B ever sees a
  discrepancy it cannot explain.
