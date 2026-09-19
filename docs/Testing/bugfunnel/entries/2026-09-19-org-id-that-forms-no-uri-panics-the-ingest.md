---
id: 2026-09-19-org-id-that-forms-no-uri-panics-the-ingest
date: 2026-09-19
gap: COVERAGE
status: FIXED
summary: >-
  An org file carrying `:ID: my task` in a headline drawer, or `#+ID: my doc`
  at the top, unwound inside `EntityUri::new` during parse — every vault file
  is author-written, so one typed space in an id crashed the whole ingest
  rather than refusing one file.
---

## Bug

Found by a VERIFIER reading the `block-ctor-sweep` lane's diff
(`lane-logs/block-ctor-sweep-verify.md`), which refuted that lane's
completeness claim: the same panicking constructor is reachable from a far
wider door than the render builders the lane had swept. No user reported it;
no suite drew it.

Org files are the vault's own storage and are written by hand, so every `:ID:`
value is author text. `extract_or_generate_id`
(`crates/holon-org-format/src/parser.rs:1148-1163`) returned any non-empty
drawer value verbatim, with no validation, and the block builder then did
`EntityUri::from_raw(&id)` (`:959`). `from_raw` falls back to
`EntityUri::block`, which is `EntityUri::new("block", id)`, which panics:

```
thread 'parser::tests::parse_rejects_a_heading_id_that_forms_no_uri' panicked at
crates/holon-api/src/entity_uri.rs:134:13:
EntityUri::new("block", "my task") produced invalid URI: unexpected character at index 8
```

The document's own carrier had the identical hole one level up:
`resolved.map(|bare| EntityUri::block(&bare))` (`:396`) took the parsed
`#+ID:` straight into the same constructor, so `#+ID: my doc` on line 1
panicked before any headline was read.

Both reds are logged at `lane-logs/d3-red.log:30` and `:50`.

## Root cause

The file already had the right disposition for a bad id and did not apply it
to this axis. Two other id faults in the same parser refuse the file loudly
and say why: an id collision between a heading and the document
(`reject_id_cycles`, `:517-553`) and a probe/parse divergence on the document
id (`:381-393`). Both `anyhow::bail!` so the file is quarantined at ingest.

An id that forms no URI is the same kind of fault — content, in one file, that
cannot become a block id — but it had no check, so it fell through to the
constructor and became a process-wide panic instead of a file-scoped refusal.
Minting a replacement id would have been worse: the heading would be filed
under an id the file does not name, which is the data loss the probe/parse
guard at `:381` exists to prevent.

## Missing piece

No test fed the parser an id that forms no URI. The parser's id-fault tests
(`parse_rejects_duplicate_heading_ids`, `parse_rejects_doc_id_equal_to_heading_id_self_parent`)
cover collisions only, and both use well-formed ids.

The keystone cannot draw it either, and that is the coverage gap: the
org-write transitions use the fixed `GEN_PLACEHOLDER`
(`crates/holon-integration-tests/src/pbt/transitions/write_org_file.rs:89`,
`:143`, `:333`) and the mutation generators mint `block-{n}` / `gen-{n}`
(`generators.rs:1029`, `create_block_under_focus.rs:139`), so no drawn id can
contain a space. Widening that alphabet is a keystone-generator change that
every downstream oracle must agree with; it is DEFERRED and flagged on the
sibling entry `2026-09-19-live-block-id-that-forms-no-uri-kills-the-window`.

## Remedy

FIXED (lane `block-ctor-sweep`, delta 2). Both id carriers now refuse the file
through the parser's existing `Result` path, with a message naming the file,
the carrier and the value:

- the headline `:ID:` is checked with `EntityUri::try_from_raw` at the point of
  extraction, where `file_id` and the headline title are both in scope; and
- the document `#+ID:` is parsed with `EntityUri::parse(&format!("block:{bare}"))`,
  which is `EntityUri::block` minus the panic, so the accepted set is unchanged.

Pinned by `parse_rejects_a_heading_id_that_forms_no_uri` and
`parse_rejects_a_document_id_that_forms_no_uri` in
`crates/holon-org-format/src/parser.rs`, both asserting the message names the
id, and both red first at `lane-logs/d3-red.log`.
