---
id: 2026-09-26-multi-line-property-value-corrupts-org-drawer
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A block write whose property value held a line break was accepted, and the
  org write-back wrote the value raw into the property drawer: the drawer
  closed early and the rest of the value became a heading and a drawer of its
  own, so the block lost its id on the next parse.
---

## Bug
Found by the adversarial verifier of the decision block adapter (lane
`decision-block-adapter`, `lane-logs/inc5-verify.md`, defect D1): a ruling
note `line one\n* Evil heading\n:PROPERTIES:\n:ID: hijack\n:END:` written as a
`note` property made the decision block re-mint its id on re-parse and moved
its children one level up. The cause is not decision-specific: any
`create` / `set_field` / `update` of a block property carried the value to
the store and to the org file unchecked.

Reproduced through the real engine and org write-back in
`crates/holon-integration-tests/tests/editing_suite/multiline_property_write_boundary.rs`
(red log `lane-logs/inc5-r2-engine-red.log`): after a `create` with that
`note`, `notes.org` holds

```
** child
:PROPERTIES:
:ID: n-child
:note: line one
* Evil heading
:PROPERTIES:
:ID: hijack
:END:
:END:
```

## Root cause
The org renderer writes each drawer property as `:key: value` with the value
verbatim (`crates/holon-org-format/src/org_renderer.rs`, `prepare_block_for_org`),
and an org drawer line cannot hold a line break. Nothing between the intent
and the store refused such a value: `BlockWriteField::parse` types the key,
not the value.

## Missing piece
The keystone cannot generate the interaction: `ApplyMutation`'s UI/Action
sources are gated out of the composed alphabet
(`crates/holon-integration-tests/src/pbt/transitions/apply_mutation.rs` header),
and the External source goes through the org file, which cannot carry a
multi-line property at all. No transition dispatches a property write with a
free-text value through the engine.

## Remedy
`OperationEngine::refuse_multi_line_property`
(`crates/holon/src/api/operation_engine.rs`) refuses a block `create` /
`update` / `set_field` whose user property (a `BlockWriteField::Property` key
without a `_` prefix) holds `\n` or `\r`, with an error that names the op,
the key and the block. The harness test above is green
(`lane-logs/inc5-r2-engine-green.log`). No keystone row: the keystone has no
engine-dispatched property write to carry it.
