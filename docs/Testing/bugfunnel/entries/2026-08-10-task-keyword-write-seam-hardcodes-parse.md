---
id: 2026-08-10-task-keyword-write-seam-hardcodes-parse
date: 2026-08-10
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Every task-keyword WRITE seam hardcodes `TaskKeywordVocabulary::default()`,
  but the PARSE seam honors the document's own `#+TODO:` line — so in any
  document declaring a custom vocabulary the store and the org file disagree
  about which blocks are tasks, in both directions.
source_line: 745
---

## Bug

(task #67 triage lane, found by agent investigation while re-verifying a
finding #64's verifier had flagged; no automated test produced it) **Every
task-keyword WRITE seam hardcodes `TaskKeywordVocabulary::default()`, but
the PARSE seam honors the document's own `#+TODO:` line — so in any document
declaring a custom vocabulary the store and the org file disagree about
which blocks are tasks, in both directions.**
`TaskKeywordVocabulary::for_document` has zero production callers. (B1)
under-promotion: typing `NEXT call bank` under `#+TODO: NEXT WAITING \ |
DONE` leaves `task_state` NULL while the rendered `* NEXT call bank` is a
task to Emacs, LogSeq and Holon's own parser — and the #30 re-ingest
reconciler correctly suppresses the re-derivation, so the disagreement is
permanent and task queries miss the block. (B2) silent demotion, the
data-mutating direction: typing `TODO buy milk` in that same document DOES
promote (default vocabulary), then re-ingest under the doc vocabulary reads
`TODO` as ordinary text — `reconcile_idempotent_reingest` returns `None` on
its first line (it guards only the opposite direction) and the ingest
overwrites the row, demoting the user's task and putting the keyword back
inside their text. NOTE the ORIGINAL #64 finding — typed `TODO milk` staying
unpromoted in `block_raw` — is DISSOLVED on `e233a9e4`; the landed live
promotion fixed it, proven in
`docs/Testing/task67-vocabulary-asymmetry-probe.txt`.

## Root cause

task #67 triage lane, found by AGENT INVESTIGATION — task #64's verification
flagged a "block_raw vs org projection disagree about whether these bytes
are a task" asymmetry; re-verified on main `e233a9e4` it turned out the
ORIGINAL framing is DISSOLVED for the typed path (the landed live promotion
fixed it) but the same asymmetry is STILL reachable by ordinary typing in
any document that declares its own `#+TODO:` vocabulary, in BOTH directions,
and one direction is silent user-data mutation: **every WRITE seam hardcodes
`TaskKeywordVocabulary::default()`; only the PARSE seam honors the
document's declared `#+TODO:` line.** `TaskKeywordVocabulary::for_document`
exists (`crates/holon-org-format/src/task_keyword.rs:44`) and has ZERO
production callers — the promotion trigger
(`crates/holon-frontend/src/editor_view_model.rs:381`), the engine's
compound (`crates/holon/src/api/operation_engine.rs:1110`) and the keystone
ref model
(`crates/holon-integration-tests/src/pbt/transitions/type_chars.rs:139`) all
pass `::default()` (`TODO DOING LATER NOW` / `DONE CANCELLED CLOSED`), while
`parse_todo_keywords_config` (`crates/holon-org-format/src/parser.rs:74`)
feeds the file's own vocabulary to the parser. TWO reachable divergences,
both probe-proven verbatim
(`docs/Testing/task67-vocabulary-asymmetry-probe.txt`). (B1)
UNDER-PROMOTION: in a doc declaring `#+TODO: NEXT WAITING | DONE`, typing
`NEXT call bank` does not promote — store holds content `NEXT call bank` /
`task_state` NULL, write-back renders `* NEXT call bank`, and re-parsing
those same bytes under the document's own vocabulary yields content `call
bank` / `task_state` `NEXT`. Emacs, LogSeq and Holon's own parser all read
the file as a task; Holon's store never will, because the #30 re-ingest
reconciler (`crates/holon-orgmode/src/file_format.rs:126`) correctly
suppresses the re-derivation — so the two layers disagree PERMANENTLY and
every task query silently misses the block. (B2) SILENT DEMOTION — the
data-mutating direction, and the #30 guard structurally CANNOT catch it: in
that same doc, typing `TODO buy milk` DOES promote (the default vocabulary
has `TODO`), so the store holds content `buy milk` / `task_state` `TODO` and
renders `* TODO buy milk`; on the next re-ingest the doc-vocabulary parser
reads `TODO` as ordinary text, giving content `TODO buy milk` / `task_state`
NULL. `reconcile_idempotent_reingest` returns `None` at its very first line
(`file_format.rs:127-129` — it guards ONLY stored-plain→parsed-promoted),
and `crates/holon-filesystem/src/file_sync_controller.rs:3345-3353` then
overwrites the row from the parsed block unconditionally (`content_differs`
sees both fields differ; `task_state` is never merged, only replaced). The
user's task is silently demoted AND the keyword reappears inside their text
— the exact class #30 was filed for, in the mirror direction. Classified
COVERAGE (primary): the interaction is ordinary typing and runs in the
keystone's own wiring, so it is neither ENVIRONMENT nor PERCEPTION; the
escape is pure generation — no draw ever mints a document carrying a
`#+TODO:` header, so the vocabulary axis has never been exercised at all.
Secondary ORACLE, and this is the sharp part: the keystone's own ref model
at `type_chars.rs:139` hardcodes the SAME `::default()` constant as prod, so
even with the coverage arm added the oracle would agree with prod's wrong
answer — a model that copies the SUT's constant cannot convict it. Closing
this needs BOTH: a generator arm minting `#+TODO:` documents AND a ref model
that derives its vocabulary from the drawn document. TRIAGE ONLY — no fix in
this lane; recommended direction is to thread the document's vocabulary to
the write seams (`for_document` at the call sites above, read from the
block's document row) rather than to widen the reconciler, because widening
it would make a genuine Emacs-side demotion unappliable. NOT closed, NOT
counted twice: the ruled `set_field` asymmetry (P1=A, agents use explicit
task ops) and the deliberate `restore_refused_promotion` verbatim-commit
both land in this same state and are SHIPPED behavior, not defects.)

## Missing piece

COVERAGE: the interaction is ordinary typing in the keystone's own wiring,
so neither environment nor perception; no draw has ever minted a document
carrying a `#+TODO:` header, so the vocabulary axis is entirely ungenerated.
ORACLE (secondary, and load-bearing): the keystone ref model at
`crates/holon-integration-tests/src/pbt/transitions/type_chars.rs:139`
hardcodes the SAME `::default()` constant as prod, so adding the coverage
arm alone would leave the oracle agreeing with prod's wrong answer. Missing
piece = a generator arm minting `#+TODO:` documents PLUS a ref model
deriving its vocabulary from the drawn document.

## Remedy

FIXED 2026-08-10, same day, by the task #68 Inc 5 lane: the recommended
direction was taken exactly — a narrow engine-side `TaskVocabularySource`
threads the owning document's declared vocabulary to the write seams as a
read-at-use argument (no cache, matching the #64 design), the reconciler was
NOT widened (a genuine Emacs-side demotion stays appliable), and the ref
model carries per-document vocabulary. B2 proven end-to-end red-first by
driving the REAL `FileSyncController` re-ingest with a custom-`#+TODO:`
document (the S3 link this row disclosed as code-read-only is now a
demonstration, `lane-logs/task68/inc5-red.txt`); the regression LOCK is the
S2/S3 pair in `crates/holon/tests/promote_task_keyword_compound.rs`
(verifier-confirmed to red on a forced `::default()`; the reingest tests
supply a test-side vocabulary and are deliberately not the lock). STILL OPEN
within this class: the keystone generator arm minting `#+TODO:` documents +
the vocabulary-aware ref draw (GAP NOT CLOSED, deliberate cut). Original
triage text follows. Recommended direction: thread the owning document's
vocabulary to the write seams via the existing
`TaskKeywordVocabulary::for_document`
(`crates/holon-org-format/src/task_keyword.rs:44`) at
`crates/holon-frontend/src/editor_view_model.rs:381`,
`crates/holon/src/api/operation_engine.rs:1110` and the ref model, rather
than widening `reconcile_idempotent_reingest` — widening the reconciler
would make a genuine Emacs-side demotion unappliable. Overlaps task #68's
Inc 5 (document `#+TODO:` vocabulary through the same read-at-use seam),
which should absorb it. Explicitly NOT defects and not counted: the ruled
`set_field` non-promotion (P1=A) and the deliberate verbatim commit in
`restore_refused_promotion` both produce the same shape as shipped behavior.
