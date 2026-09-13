---
id: 2026-09-13-readonly-gate-parses-block-id-only-when-a-readonly-doc-exists
date: 2026-09-13
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  The write-tier gate parses a block operation's id only when the vault happens
  to hold a read-only document, so the same write is refused on one vault and
  accepted on another.
---

## Bug

Dispatch `block/set_field` with an UNSCHEMED block id — the form
`docs/Reference/ORG_SYNTAX.md` says vault files carry. What happens depends on
a property of the vault that has nothing to do with the write:

| Vault | Outcome |
|---|---|
| holds ≥ 1 read-only document | refused: `Operation 'set_field' on entity 'block' failed: Invalid URI "storedid0": unexpected character at index 9` |
| holds none | accepted; the write lands on the right block |

Neither outcome is wrong on its own — refusing an id the boundary does not
accept is fail-loud, and accepting it is the documented normalization. Having
BOTH, selected by unrelated vault state, is the defect. The error message also
names a URI the user never wrote and does not mention read-only documents, so
it points nowhere near its cause.

Found while building a keystone rung for
[2026-09-13-undo-precondition-reader-finds-no-content](2026-09-13-undo-precondition-reader-finds-no-content.md)
(lane `undo-precondition`): the rung passed on a bare vault and panicked on the
wide keystone seed, which contains a read-only document.

## Root cause

`ReadOnlyFormatGate::refusal_for`
(`crates/holon-app/src/read_only_format_gate.rs:44-51`):

```rust
async fn refusal_for(&self, block_id: &str) -> Result<Option<EditRefused>> {
    if self.documents.is_empty() {
        return Ok(None);
    }
    Ok(self.documents.refusal_for_block(&EntityUri::parse(block_id)?))
}
```

The early return is a performance guard — the comment above it explains that
every rendered editable row asks this on every draw, so the membership lookup
must be cheap. The side effect is that `EntityUri::parse` — strict, no
unschemed ids — runs on the operation's id only when the guard does NOT fire.
Parsing is thus conditional on vault content. `adopt_sync_import` two lines
below has the same shape.

Measured: `lane-logs/hand-green2-1789299767.log:6538` onward (the wide seed,
booted `storage={Loro, Turso}`, refuses) against
`crates/holon-app/tests/undo_precondition_id_scheme.rs` (a temp vault with no
read-only document, accepts).

## Missing piece

COVERAGE. Nothing exercises a block operation whose id is in the unschemed form
under both vault shapes, so the divergence has no test that could see it. The
keystone always seeds a read-only document and always uses `EntityUri`-shaped
ids, which lands it on one side of the branch with an input that never trips
the parse.

Secondary ENVIRONMENT: the two shapes are different WIRINGS of the same code,
and the test fleet only ever boots one of them for this path.

## Remedy

Open. Deliberately not fixed in the `undo-precondition` lane, because the
sensible fix is the same architectural decision that lane escalated: whether
the operation boundary accepts an unschemed block id at all. Parsing the `id`
param ONCE at the dispatcher — qualifying it, or refusing it outright, before
any gate or provider sees it — makes this branch unreachable and removes the
gate's need to parse anything.

If that decision goes the other way, the local fix is to hoist the parse above
the `is_empty()` guard so the id is validated unconditionally, and to give the
refusal an error that names the read-only document.
