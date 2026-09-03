---
id: 2026-09-03-refused-write-back-still-persists-in-the-vault-loro-doc
date: 2026-09-03
gap: ORACLE
secondary: ENVIRONMENT
status: PARTIAL
summary: >-
  An edit whose write-back was refused survives in the vault's Loro document and
  is replayed into the store on every later boot, so store and disk diverge
  permanently.
---

## Bug

Found by the `dogfood-search` lane. The sibling entry
`2026-09-03-read-only-format-blocks-accept-edits-that-are-discarded` records
that a `.cook` recipe step accepts an edit which never reaches disk. The word
"discarded" turns out to be wrong: the edit is durable.

This lane started from a **wiped** config dir and database — the whole store
was rebuilt by re-ingesting the vault copy from scratch, three times. On every
one of those boots the store came back holding text that does not exist in any
file:

| block | on disk | in the store |
|---|---|---|
| `…/Linsensuppe.cook::b::3` | `… zugeben, aufkochen und zugedeckt …` | `… zugeben, aufCQWEkochen und zugedeckt …` |
| `…/Linsensuppe.cook::b::0` | `… kalt abspülen.` | `… kalt abspülen.YZ` |

`CQWE` and `YZ` are stray keystrokes from an EARLIER dogfood session, days of
app restarts ago. The `.cook` files themselves are byte-clean.

## Root cause

The vault holds the Loro document (`.loro/holon_tree.loro` in the vault root),
and the refused write-back only stops the ORG/`.cook` leg — the CRDT leg
already committed. Confirmed directly: the vault's `holon_tree.loro` contains
`CQWE` as a live insert.

Because the Loro doc lives in the vault and not in the config dir, wiping the
config dir and the database does not remove it. Boot re-projects the CRDT over
the freshly ingested file content, so the phantom text is restored every time
and the divergence is permanent rather than session-local. Editing a
format-owned block is therefore not "an edit that goes nowhere" but "an edit
that silently forks the store away from the file, forever".

The fail-loud leg fires on the wrong side: `WRITE-BACK REFUSED` ERRORs are
logged for the file leg, while the CRDT write that actually persisted is
reported nowhere.

## Missing piece

No invariant compares stored content against file content for blocks whose
format refuses write-back. `diff_loro_sql` compares Loro against SQL — the two
that AGREE here — so it is blind by construction; the missing comparison is
Loro/SQL against **disk**.

The keystone also never edits a block owned by a read-only format, so the
sequence that creates the fork is not generated either.

## Remedy

PARTIAL — part (1), the real fix, has landed; part (2) has not.

**(1) No leg commits.** The operation dispatcher — the one writer — refuses a
block write whose owning document is homed in a `WriteTier::ReadOnly` file,
before any provider runs (`crates/holon/src/api/operation_dispatcher.rs`,
`enforce_write_tier`; the seam and the typed `EditRefused::ReadOnlyFormat` live
in `crates/holon-core/src/write_tier_gate.rs`). Since the op never reaches
`LoroBlockOperations`, no CRDT op is minted and nothing survives a wipe. The
refusal is disclosed on the degraded bus rather than only logged, so it fires on
the side where the user acted. Sibling entry:
`2026-09-03-read-only-format-blocks-accept-edits-that-are-discarded`.

Pinned by `an_edit_to_a_recipe_block_is_refused_at_the_dispatcher`
(`crates/holon-integration-tests/tests/cook_vault_ingest.rs`), which asserts the
step's content in the **write-authority store** — the Loro tree, read through
`BlockReader::get_block_authoritative`, not the lagging projection — still
matches the file after a refused edit. Red against the unfixed code with
"editing a block of a read-only-format file must be refused".

That assertion was doc-level but path-scoped: it covered `set_field`, not the
path a user's keystrokes take. The editor writes content through a
`Cell<String>` onto the block's `LoroText` directly, and ungated it minted
exactly the op this entry is about — verified by probe, `authoritative_after =
"HACKED Crack the eggs into a bowl."`. That writer now carries the dispatcher's
own decision (see the sibling entry), and
`a_keystroke_on_a_recipe_block_is_refused_at_the_cell` asserts the same
write-authority-store oracle after a `TextOp::Insert`. Both writers are covered
at the doc/ops level.

**(2) The store-vs-disk invariant is still missing.** Nothing yet compares a
block whose content came from a file against that file after settle, so a fork
arriving by some other route (a peer's `OpOrigin::Sync` edit, which the gate
deliberately lets through to keep replicas convergent) still goes unnoticed.

An already-forked vault still needs its Loro op removed; rewriting the row
leaves the CRDT op to win on the next boot.
