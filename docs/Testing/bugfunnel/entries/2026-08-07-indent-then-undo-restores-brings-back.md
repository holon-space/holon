---
id: 2026-08-07-indent-then-undo-restores-brings-back
date: 2026-08-07
gap: PERCEPTION
secondary: ORACLE
status: UNCLASSIFIED
summary: >-
  `tab` indent then `cmd-z` undo restores `parent_id` but brings `sort_key`
  back as `827F80` where it had been `8180`, which reads as silent order-key
  corruption and is not.
source_line: 1179
---

## Bug

(MCP-driven dogfood, throwaway vaults, reported INDEPENDENTLY BY TWO
SESSIONS as data corruption — registered so the family CLOSES; the re-mint
is NOT a defect) **`tab` indent then `cmd-z` undo restores `parent_id` but
brings `sort_key` back as `827F80` where it had been `8180`, which reads as
silent order-key corruption and is not.** Undo of a structural move RE-MINTS
the order key by design: `is_derived_positional_field`
(`crates/holon-core/src/undo.rs:300-309`) excludes `sort_key` from every
undo precondition because "structural ops RECOMPUTE from the live tree
rather than restore to a captured value"; the inverse carries the old
PREDECESSOR, not the old key (`move_block_op(id, old_parent_uri,
old_predecessor)`, `crates/holon-core/src/traits.rs:2203-2211`); and
`BlockOrdering::place` re-mints via `gen_key_between` against the neighbours
actually present. ORDER IS PRESERVED EXACTLY. The bytes reproduce end-to-end
from the real `loro_fractional_index`: an append-only ingest of three
children mints `80`/`8180`/`8280`, so undoing the MIDDLE child mints between
`80` and the successor that never moved (`8280`) = `827F80`, and `80 <
827F80 < 8280` is the same relative position as `80 < 8180 < 8280`.

## Root cause

secondary ORACLE: MCP-driven dogfood, reported INDEPENDENTLY BY TWO SESSIONS
as order-key corruption — `tab` indent then `cmd-z` undo restores
`parent_id` correctly but brings `sort_key` back as `827F80` where it had
been `8180`. NOT A DEFECT; registered so the family CLOSES, on the same
grounds as the org-render DEGRADED row below. Undo of a structural move
RE-MINTS the order key by design instead of restoring the captured one:
`is_derived_positional_field` (`crates/holon-core/src/undo.rs:300-309`)
excludes `sort_key` from every undo precondition because "structural ops
RECOMPUTE from the live tree rather than restore to a captured value", the
inverse carries the old PREDECESSOR and not the old key (`move_block_op(id,
old_parent_uri, old_predecessor)`,
`crates/holon-core/src/traits.rs:2203-2211`), and `BlockOrdering::place`
re-mints through `gen_key_between` against the neighbours that are actually
there. ORDER IS PRESERVED EXACTLY, and the reported bytes reproduce
end-to-end from the real `loro_fractional_index`: an append-only ingest of
three children mints `80`/`8180`/`8280`, so undoing the MIDDLE child mints
between `80` and the successor that never moved (`8280`) = `827F80`, and `80
< 827F80 < 8280` is the same relative position as `80 < 8180 < 8280`. The
org file cannot disclose it in either direction — org stores NO `sort_key`
at all (`crates/holon-orgmode/src/block_params.rs:144`,
`home_authority.rs:7`), order is positional — so a byte-identical org
round-trip is exactly what a correct re-mint predicts and is NOT evidence
that the key was restored. RECONCILES the contrary claim in the 2026-08-07
dogfood commit (63b63bf367) that undo "restores parent, sort_key and the org
file exactly": both observations are correct and differ ONLY in the block's
sibling position — undoing the LAST child mints `new_after("80")` = `8180`,
byte-identical, so the identical interaction reads as clean on one vault and
as corruption on the next, which is why this cost two independent
root-causes. No code change; locked by
`undo_remints_the_order_key_and_only_order_is_guaranteed`
(`crates/holon-core/src/fractional_index.rs`), which pins BOTH arms and the
exact observed values so a third sighting is answered by a test instead of
an investigation)

## Missing piece

The observation surface cannot express the contract: the raw `sort_key`
bytes are the only thing an MCP/SQL observer sees, and they are precisely
the part that is deliberately free, so a benign re-mint and a real reorder
look identical at the surface. Secondary ORACLE because nothing anywhere
stated the actual contract (order restored, bytes free) — which is how the
2026-08-07 dogfood commit 63b63bf367 came to assert the opposite ("undo of a
UI-driven indent restores parent, sort_key and the org file exactly").
RECONCILED, not contradicted: both observations are correct and differ ONLY
in sibling position — undoing the LAST child mints `new_after("80")` =
`8180`, byte-identical, so the identical interaction reads clean on one
vault and corrupt on the next. The org file is silent either way: org stores
NO `sort_key` (`crates/holon-orgmode/src/block_params.rs:144`,
`home_authority.rs:7`), order is positional, so a byte-identical org
round-trip is what a CORRECT re-mint predicts and is not evidence the key
was restored.

## Remedy

NO CODE CHANGE — the design is right and the arithmetic is right. Locked
2026-08-07 by `undo_remints_the_order_key_and_only_order_is_guaranteed`
(`crates/holon-core/src/fractional_index.rs`), which pins both arms and the
exact observed values (`80`/`8180`/`8280`/`827F80`) against the real
fractional-index implementation, so a third sighting is answered by a test
instead of a fresh investigation.
