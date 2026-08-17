---
id: 2026-08-10-undoing-promotion-does-survive-ingest-writeback
date: 2026-08-10
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  Undoing a promotion does not survive a re-ingest: the writeback emits `
source_line: 1195
---

## Bug

(dogfood-explorer gate on task #68, proven by a COLD BOOT on a FRESH DB
against the same vault; finding F2) **Undoing a promotion does not survive a
re-ingest: the writeback emits `** TODO alpha four` for a block the user
demoted, so the next full ingest silently re-promotes it — and when the
content was only the keyword, re-ingest also ERASES the typed word
(`content` becomes `""`, `task_state=TODO`).** Root shape: undo restores
"plain text that begins with a keyword of this document's vocabulary", a
state org cannot natively express; the writeback emits the ambiguous
headline with no escaping and no disclosure, so the round trip is lossy in
the data-mutation direction. Per the project's fail-loud priority order this
sits at the forbidden tier 4 (silently degrades to look fine). The #68 B2
fix covers only keywords OUTSIDE the vocabulary, where the ambiguity never
arises. METHOD NOTE, load-bearing for whoever closes this: the watcher's
diff-based re-ingest after an external file edit does NOT re-parse unchanged
blocks, so an append-driven test FALSE-PASSES — only a fresh-DB cold boot
proves it.

## Missing piece

no round-trip invariant on the UNDONE state (promote → undo → render →
re-ingest ⇒ identical block), and no cold-boot fresh-DB re-ingest rung at
all — the existing re-ingest tests are append-driven and structurally cannot
see this

## Remedy

FIXED 2026-08-10 (task #78) by Martin's EAGER-CONVERGENCE ruling at BROAD
scope, recorded here because the fix is a semantics decision and the row is
where it is discoverable: **"plain text whose content begins with a keyword
of the document's own vocabulary" is an ILLEGAL STATE**, not a state to
escape (option i), disclose (option ii) or normalize-with-a-warning (option
iii, this row's recommendation — OVERRULED). It converges to `task_state` +
stripped content IMMEDIATELY, at the store write boundary, on EVERY write
path — set_field, create, agent/MCP writes, undo/redo replay and the
promotion compound alike. WYSIWYG: the store never holds a reading of bytes
that org disagrees with, so no banner and no escape syntax is needed and the
round trip is a fixed point by construction. IMPLEMENTATION: the rule is a
pure function of (content, document vocabulary) —
`holon_org_format::converge_keyword_headed`, which REPLACED the delta-shaped
`detect_keyword_promotion` — applied by `OperationEngine` at its two write
seams (`execute_operation`, and `replay` for undo/redo), routed through the
same read-at-use `TaskVocabularySource` seam task #68 built, so a format
provider that declares no keywords converges nothing and stays a pure
plug-in. The ingest path converges in the PARSER, which already reads
keywords under the document's own vocabulary — which is why the #30
re-ingest reconciler (`FileFormatAdapter::reconcile_idempotent_reingest`,
its org override and its `file_sync_controller` call site) is DELETED
tree-wide rather than kept as a legacy path: it existed only to protect the
state that is now illegal. RULED CONSEQUENCES, each pinned: (a) the P1=A pin
`set_field_never_promotes_however_the_text_looks` is RETIRED and rewritten
as `set_field_converges_however_the_text_arrives` (the ruling reversal is
stated in its doc comment); (b) `reingest_task_promotion_idempotent.rs`
asserts the NEW fixed point — keyword-headed bytes ingest as a task, and a
SECOND re-ingest changes nothing; (c) undo of a promotion is semantically
VOID and reports itself so: the content inverse still restores the verbatim
typed text (asserted at the journal layer), the store converges it straight
back, and `undo()` returns `NoChange` — the escape is further undos through
the typing ops underneath. The round-trip invariant this row asked for is
satisfied in its STRONGEST form rather than restated to admit a disclosed
normalization: promote → undo → render → re-ingest yields the identical
block, because the undone state is the promoted state. The row's METHOD NOTE
still holds and is why the reingest rung now runs TWO ingests. The
per-document half is pinned by
`set_field_converges_by_the_documents_own_vocabulary` (NEXT converges under
`#+TODO: NEXT WAITING \ | DONE`, TODO does not) and by
`document_vocabulary_is_authoritative`, which also pins the empty-vocabulary
plug-in arm.
