---
id: 2026-08-22-importer-ignores-the-transaction-tail
date: 2026-08-22
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Holon's LogSeq-DB importer read only the B+-tree rows and skipped the
  transaction tail at kvs addr 1, so a graph LogSeq had edited recently
  imported as its pre-edit self with no error anywhere.
---

## Bug
Found in W1 (lane `lsqdb-import`, 2026-08-22) by the acceptance test for the
first Holon-authored edit. Holon wrote a `:block/title` replacement into the
tail; the byte-level tests confirmed the write landed, and LogSeq's own
validator and graph diff both SAW the change — but Holon's own importer
re-read the edited graph and reported `BaseDiff { created: [], changed: [],
removed: [] }`. Nothing changed, as far as Holon was concerned.

The immediate trigger was Holon's own write, but the defect is not about
writing. Any graph LogSeq itself has edited recently imports the same way:
stale, silently.

## Root cause
DataScript does not rewrite its index trees for a small transaction. Datoms
are appended to a TAIL at kvs addr 1 and stay there until they exceed the
branching factor; every reader replays that tail over the trees
(`storage.cljs restore-impl` + `db-with-tail`, reached from `restore-conn`,
which is what LogSeq's own `sqlite-cli/get-storage-conn` calls).

`read_datoms` (`crates/holon-logseq-db/src/datoms.rs`) walked every row with
`addr > 0` and pulled datoms out via `leaf_tuples`. A tree node is a Transit
MAP with a `:keys` entry; the tail is a Transit LIST. So `leaf_tuples` hit its
`let TransitNode::Map(entries) = node else { return &[] }` arm and returned
nothing — the tail was skipped by the same code path that legitimately skips a
node with no keys. No error, no warning, no count mismatch: the tail simply was
not there.

Measured on the fixture: the committed `holontest.sqlite` has row 1 = `[]`, an
empty tail, so every test that had ever run imported a graph where the bug was
invisible by construction.

## Missing piece
No input with a non-empty tail existed. Every test in the crate imports the one
committed fixture, and that fixture's tail is empty — so no transition sequence
available to any test could reach the state where the tail matters. That is
what makes this COVERAGE rather than ORACLE: the assertions were not too weak,
the input space had a hole in it.

The secondary ORACLE gap is real too: until W0 there was nothing comparing
Holon's reading of a graph against LOGSEQ's reading of the same graph, so a
silent staleness had nothing to contradict it. The three-leg harness is what
turned "Holon sees nothing" into a failing test rather than a quiet wrong
answer.

The keystone `general_e2e_composed_pbt.rs` cannot reproduce this — it does not
drive the LogSeq-DB importer — so no keystone red is available or expected.

## Remedy
FIXED. `apply_tail` in `crates/holon-logseq-db/src/datoms.rs` replays the tail
over the datoms restored from the trees: entries are applied in order, an
assertion inserts under the tail's transaction id, and a retraction removes the
datom with that `(e, a, v)` whatever transaction first asserted it. Addr 1 is
now excluded from the tree-scanning loop by name rather than falling through
`leaf_tuples`, so the skip is deliberate instead of accidental. An unreadable
tail is `ImportError::Tail`, never an empty one — importing a stale graph
silently is the exact failure this replaces.

The gap is closed at the input end as well: the W1 write path can now produce a
graph with a non-empty tail, and
`the_edit_shows_up_as_exactly_one_changed_block`
(`crates/holon-logseq-db/tests/kvs_round_trip.rs`) is the regression pin —
it writes a real edit into the tail and requires the importer to report exactly
one changed block. Red before the fix with the diff quoted above.

Related hazard, recorded because it makes this class of bug hard to see:
`db-with-tail-datoms` wraps its replay in `(catch :default _ db)`, so LogSeq
DROPS a tail transaction it cannot parse and loads the graph as though the edit
never happened. Holon therefore refuses any tail entry not shaped `[e a v ±tx]`
at parse time (`RowError::MalformedTailDatom`), and `just lsqdb-oracle`'s
exact-delta leg is mandatory on every increment that writes — it is the only
check that would catch a well-formed-but-wrong tail.
