---
id: 2026-09-26-multi-line-property-value-corrupts-org-drawer
date: 2026-09-26
gap: COVERAGE
secondary: null
status: PARTIAL
summary: >-
  A property value that holds a line break reaches the org write-back raw:
  the drawer closes early, the rest of the value becomes a heading and a
  drawer of its own, and the block loses its id on the next parse. The engine
  refuses it now; the peer-merge and foreign-ingest legs and the renderer
  itself still let it through.
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
Mitigated at one seam only. `OperationEngine::refuse_multi_line_property`
(`crates/holon/src/api/operation_engine.rs`) refuses a block `create` /
`update` / `set_field` that puts a line break into a drawer text: a loose
user property, any text value in the `properties` bag (object or JSON
string, also as the `set_field` field), and the `org_properties` /
`file_properties` drawer carriers. Typed values (`Json`, `Object`, `Array`,
`DateTime`) never reach the drawer
(`crates/holon-org-format/tests/typed_property_values_stay_out_of_the_drawer.rs`).

Still open, for a separate lane:
- Loro / peer merge: a peer or an older build that stores a multi-line
  property never meets the engine, and its value reaches the org write-back.
- Foreign ingest: Markdown/Obsidian ingest sets properties on the `Block`
  directly (`crates/holon-markdown/src/obsidian.rs`) and bypasses the engine.
- The renderer is the last line: it should refuse or escape a value it cannot
  write, whatever path the value took.
