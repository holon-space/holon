---
id: 2026-07-18-severe-data-loss-templates-make-discoverable
date: 2026-07-18
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  SEVERE DATA LOSS (templates make-discoverable, found by live verifier):
  `instantiate_template` deep-copies the template's org `:ID:` property into
  EVERY instance block. The org parser lifts `:ID:` into an `"ID"` property
  (`crates/holon-orgmode/src/block_params.rs:157`), and `plan_instantiation`
  (`crates/holon/src/core/template_instantiation.rs`) copied all properties
  minus `_provenance` — so an instance was created with
  `properties={"ID":"tpl-daily","instance_of":"block:tpl-daily"}`. On org
  writeback+reload the duplicate `:ID:` collides on the DERIVED block id
  (writeback emits `:ID:` from the id, and two blocks claiming `tpl-daily`
  collapse on reparse): the verifier observed `Templates.org` EMPTIED
  (template content gone, file reduced to the `#+ID:` header) and the template
  block corrupted into an instance of ITSELF.
source_line: 1010
---

## Bug

SEVERE DATA LOSS (templates make-discoverable, found by live verifier):
`instantiate_template` deep-copies the template's org `:ID:` property into
EVERY instance block. The org parser lifts `:ID:` into an `"ID"` property
(`crates/holon-orgmode/src/block_params.rs:157`), and `plan_instantiation`
(`crates/holon/src/core/template_instantiation.rs`) copied all properties
minus `_provenance` — so an instance was created with
`properties={"ID":"tpl-daily","instance_of":"block:tpl-daily"}`. On org
writeback+reload the duplicate `:ID:` collides on the DERIVED block id
(writeback emits `:ID:` from the id, and two blocks claiming `tpl-daily`
collapse on reparse): the verifier observed `Templates.org` EMPTIED
(template content gone, file reduced to the `#+ID:` header) and the template
block corrupted into an instance of ITSELF.

## Missing piece

No test instantiated an ORG-PARSED template (the only path that carries the
`"ID"` property) and round-tripped it through writeback+reload — the
`instantiate_template_tests` seed templates via `create_block` with no
`"ID"` property, so the ID-copy was never exercised; AND the ONE composed
keystone PBT (`general_e2e_composed_pbt`) has no `instantiate_template`
transition, so it cannot generate the interaction at all. ORACLE secondary:
no invariant asserts an instance carries no identity-colliding property, nor
at-most-one block per `:ID:` after a writeback→reload cycle (the
org-roundtrip destruction class).

## Remedy

FIXED — `plan_instantiation` now strips an explicit denylist
`NON_COPYABLE_PROPERTIES = ["ID", "_provenance"]` from every node
(parse-don't-validate: a named identity/meta denylist, not ad-hoc per-key
removes; template `template`/`template_vars` markers stay root-only stripped
so nested templates round-trip). Two RED-before tests: unit
`plan_excludes_org_identity_property_from_every_instance` (asserts
create-params properties exclude `ID`, non-identity props still copy,
instance id ≠ template `ID`); end-to-end engine
`instance_never_carries_template_org_id` (seeds a template with an `"ID"` in
its `properties`, instantiates through the real `TursoTemplateSource`+create
path, asserts the instance row's properties have no `ID`). Keystone repro
deferred: needs an `instantiate_template` transition + an
`inv-no-duplicate-block-id-across-writeback` invariant (COVERAGE+ORACLE
remedies) — not added this workstream (make-discoverable is frontend-scoped;
the keystone base carries pre-existing REDs).
