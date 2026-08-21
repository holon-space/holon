---
id: 2026-08-22-org-file-step-drops-declared-todo-ring
date: 2026-08-22
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  The `Given an org file "x":` step silently dropped a docstring's `#+TODO:`
  header, so a hand-authored scenario that declared its own task-keyword ring
  replayed under the parser DEFAULTS — and, worse, replayed corrupt: the
  parser had already resolved the declared keywords into the blocks' task
  states, which the header-less file could no longer express.
---

## Bug
Found while triaging the LogSeq-parity corpus's tasks/properties/tags cluster
(lane `gap-props`, night of 2026-08-21→22), not by a failing test.

`crates/holon-integration-tests/tests/fixtures/dogfood-recorded/task_keyword_vocabulary.feature`
records, as the session's top vocabulary gap, that a `#+TODO:`-declaring
document "cannot be authored" — that `Given an org file "custom_vocab.org":`
plus a docstring is refused whenever the org content leads with a `#+…` line,
with a verbatim `needs a docstring holding the org content` error.

That recorded diagnosis is REFUTED. The Gherkin layer preserves such a
docstring exactly. Probed by parsing a feature file whose docstring leads with
the header, and reading `step.docstring` straight off the `gherkin` crate:

```
PROBE plain      = Some("\n* HelloWorld\n")
PROBE with-hash  = Some("\n#+TODO: NEXT WAITING | DONE\n* NEXT thing\n")
```

The docstring arrives intact, `#+TODO:` and all. The refusal that session saw
must have come from a docstring that never attached (indentation / `"""`
delimiters), not from its content.

The REAL defect sits one layer down, and is silent rather than loud.

## Root cause
`WriteOrgFile::from_org_text` — the only path a feature-file author can reach —
hardcoded `keyword_set: None`
(`crates/holon-integration-tests/src/pbt/transitions/write_org_file.rs`, the
`Ok(Self { … })` at the end of the function).

The field is consumed on BOTH legs, so dropping it desynchronises them from the
author's text:

- ref leg: `apply_to_ref` passes `self.keyword_set.as_ref().map(|ks| ks.0.clone())`
  into `seed_org_file` — seeds `None`, i.e. the default ring.
- SUT leg: the `cap_transition!` body re-emits the header only under
  `match &me.keyword_set { Some(ks) => format!("{}\n{}", ks.to_org_header(), content), None => content }`
  — so the file written to the watched dir carries NO `#+TODO:` line.

This is corruption, not mere loss. `parse_org_file` DOES honour the header
while parsing, so the blocks it returns already carry the declared keywords as
resolved task states. The replay then writes those blocks to a file that no
longer declares them, and production's re-ingest cannot resolve them back —
the exact failure mode the SUT leg's own comment warns about ("without the
`#+TODO:` header they'd re-parse as headline content instead of task states").

Red-for-the-right-reason, from the catalog-wide round-trip law
`step_vocabulary_laws::parse_of_render_is_the_identity`:

```
WriteOrgFile rendered the step "an org file \"custom_vocab.org\":", which
parses back to a DIFFERENT value
  left:  … "blocks": [ … "content": "thing", "properties": {"task_state": "NEXT", …} ],
         "keyword_set": [{"keyword":"NEXT","category":"Active"},
                         {"keyword":"WAITING","category":"Active"},
                         {"keyword":"DONE","category":"Done"}]
  right: … "blocks": [ … "content": "thing", "properties": {"task_state": "NEXT", …} ],
         "keyword_set": Null
```

The blocks match exactly — `content: "thing"` with `task_state: NEXT` proves
the parser consumed the header — and only the ring itself is `Null`. That is
the corruption in one frame: the state survives, its vocabulary does not.

## Missing piece
ORACLE (primary). The law that exists to catch a lossy step vocabulary —
`parse(render(t)) == t` over `step_catalog_examples()` — was deliberately
made vacuous for this exact field. `WriteOrgFile::step_examples()` carried:

```
// Keyword-set-carrying examples are deliberately absent: the parse side
// reads the org text with the production parser, which resolves the
// `#+TODO:` header into task states rather than handing the set back.
```

Every other variant's examples exercise every field; this one exempted the only
field that was lossy, so the law passed while the vocabulary silently corrupted.
The neighbouring test `a_stray_docstring_is_a_loud_refusal` states the intended
contract — "author intent the vocabulary cannot honour — refused, never
dropped" — which this exemption quietly suspended.

COVERAGE (secondary). No fixture ever seeded a declaring document, so nothing
replayed the corrupt file. The generator-side property
`keyword_set_survives_sut_serialize_parse` covers only generator-produced
values, never the `from_org_text` path an author reaches.

## Remedy
FIXED, in `write_org_file.rs`.

The ring is now carried out of the document block the production parser
already resolved it into, rather than re-parsed from the text:

```rust
let keyword_set = holon_org_format::models::OrgDocumentExt::todo_keywords(&parsed.document)
    .map(crate::pbt::generators::TodoKeywordSet);
```

`OrgDocumentExt::todo_keywords()` is production's own accessor (the org
write-back path reads it through `sync_document_metadata`), so the step
vocabulary and production agree on what a `#+TODO:` line means by construction
— no second header parser to drift.

Gap closed on the oracle side by deleting the exemption: `step_examples()` now
contributes a DECLARING document, so `parse_of_render_is_the_identity` covers
`keyword_set`. The example states the ring explicitly rather than echoing
`from_org_text`, so the law checks the fix rather than agreeing with it.

Follow-on effect: the custom-vocabulary arm of `task_keyword_vocabulary.feature`
is no longer blocked. That feature's "gap 1" note should be rewritten — a
declaring document IS authorable from a docstring — but that is a separate
change and is left to the lane that owns that file.
