---
id: 2026-09-26-multi-line-property-value-corrupts-org-drawer
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A property value that holds a line break reaches the org write-back raw:
  the drawer closes early, the rest of the value becomes a heading and a
  drawer of its own, and the block loses its id on the next parse. The org
  codec now writes such a value as one JSON string literal and reads it back
  byte-equal, on every write path.
---

## Bug
Found by the adversarial verifier of the decision block adapter (lane
`decision-block-adapter`, `lane-logs/inc5-verify.md`, defect D1): a ruling
note `line one\n* Evil heading\n:PROPERTIES:\n:ID: hijack\n:END:` written as a
`note` property made the decision block re-mint its id on re-parse and moved
its children one level up. The cause is not decision-specific. The second
verifier (`lane-logs/inc5-r2-verify.md`, R2-D1) reproduced the same file
damage through the `properties` bag of a `create`, which the first engine
guard did not inspect.

Reproduced through the real engine and org write-back in
`crates/holon-integration-tests/tests/editing_suite/multiline_property_write_boundary.rs`
(red logs `lane-logs/inc5-r2-engine-red.log`, `lane-logs/inc5-r3-red.log`):
after a `create` with that `note`, `notes.org` holds

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
The org renderer writes each text drawer property as `:key: value` with the
value verbatim (`crates/holon-org-format/src/org_renderer.rs`,
`prepare_block_for_org`; `drawer_properties` in
`crates/holon-org-format/src/models.rs`), and a drawer line cannot hold a line
break. Nothing on any write path refuses such a value, and the renderer does
not refuse or escape it either.

## Missing piece
The keystone cannot generate the interaction: `ApplyMutation`'s UI/Action
sources are gated out of the composed alphabet
(`crates/holon-integration-tests/src/pbt/transitions/apply_mutation.rs` header),
and the External source goes through the org file, which cannot carry a
multi-line property at all.

## Remedy
Fixed at the point every write path meets: the org value codec
(`crates/holon-org-format/src/drawer.rs`, `ValueCarrier`; rule in
`docs/Reference/ORG_SYNTAX.md`). The renderer writes a value that its parser
would not read back unchanged as a JSON string literal on one line, and the
parser decodes only the exact literal the renderer writes. This covers the
headline drawer, the file-level drawer, the dense projection (same drawer
formatter) and source-block header arguments, so the engine, Loro/peer merge,
Markdown/Obsidian ingest and every other seam reach the file through the same
rule. The engine guard `refuse_multi_line_property` is removed; the engine now
refuses only a property KEY org cannot hold (`DrawerKey`).

Pinned by:
- `crates/holon-org-format/tests/drawer_value_codec_pbt.rs` (render -> parse
  identity on all three carriers; user-typed values written back as typed).
- The keystone: the External `update_custom_prop` generator
  (`drawer_value_strategy` in `crates/holon-integration-tests/src/pbt/generators.rs`)
  draws multi-line, padded, empty and quoted values, and the hand-authored
  cases `external-drawer-values-round-trip-{loro,sqlonly}-arm` replay the
  injection deterministically (red log `lane-logs/groupA-red-keystone-hand-authored.log`:
  the injected `:ID: hijack` minted a real block).
- `crates/holon-integration-tests/tests/editing_suite/multiline_property_write_boundary.rs`
  now asserts the engine write shapes are accepted and the file carries the
  value byte-equal.
