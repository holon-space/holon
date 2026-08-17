---
id: 2026-08-10-undoing-promotion-whose-whole-typed-text
date: 2026-08-10
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Undoing a promotion whose whole typed text was the keyword restores `TODO`
  (4 bytes), not the verbatim typed `TODO ` (5) — and the lost byte is the one
  holding the anti-re-promotion guard open.
source_line: 1194
---

## Bug

(dogfood-explorer gate on task #68, live GPUI SqlOnly, seeded
DefaultVocab/CustomVocab vault; finding F5) **Undoing a promotion whose
whole typed text was the keyword restores `TODO` (4 bytes), not the verbatim
typed `TODO ` (5) — and the lost byte is the one holding the
anti-re-promotion guard open.** Localized red-first at TWO layers, which
refutes the obvious diagnosis:
`undo_of_an_empty_remainder_promotion_restores_the_consumed_space`
(`crates/holon/tests/promote_task_keyword_compound.rs`) asserts the journal
AND the column, and the journal assertion PASSES —
`undo_entry_values("inverse_ops","content") == "TODO "`, so the promotion
inverse already carries the verbatim text (`operation_engine.rs:1229-1231`).
The column assertion fails `left: Some("TODO") / right: Some("TODO ")`. The
byte dies in `holon_api::content_canonical::canonicalize_stored_content`
(`crates/holon-api/src/content_canonical.rs:26-35`, `trim_end`), reached via
`SqlOperationProvider::trimmed_content` — a DELIBERATE canonicalization
whose own comment gives two reasons (an org headline `.trim()`s on re-parse,
and the transform is the single source of truth shared with the GPUI
editor's echo-suppression discriminator, where a drift deletes typed
whitespace from the buffer). NOT cosmetic, which is why it is rated above
the dogfood report's "minor": promotion guard 3 (`task_keyword.rs:168-187`)
makes an undone promotion durable only while the restored content is itself
keyword-headed. `TODO alpha one` is; the canonicalized bare `TODO` is NOT
(`keyword_headed` requires keyword + whitespace + rest), so the
empty-remainder undo lands the block in exactly the state from which the
next space re-promotes — test G1's primary path. COVERAGE: the windowed rung
`live_promotion_windowed.rs` pins only a NON-EMPTY remainder
(`PROMOTION_TARGET_CONTENT = "milk"`), so the empty-remainder arm is
ungeneratable at every rung.

## Missing piece

the promotion rung and its seed carry one shape only (non-empty remainder);
no fixture types a keyword as the WHOLE content, and no assertion pairs the
journal's intended restore against the store's accepted restore

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
round trip is a fixed point by construction. The EMPTY-REMAINDER case is now
SPECIFIED, not accidental: bare `TODO` (and `TODO `, which the store
canonicalizer trims to it) converges to `task_state=TODO` + `content=""` —
an empty-titled task, exactly what `** TODO` means on disk. The lost
trailing byte becomes INERT: it mattered only because it held the old guard
3 open, and guard 3 no longer exists — `keyword_headed` now admits
keyword-then-end-of-string, so `TODO` and `TODO ` converge alike and no
state re-fires. `canonicalize_stored_content` is UNTOUCHED, so the GPUI
echo-suppression discriminator it is the single source of truth for keeps
its contract. Pinned by
`an_empty_remainder_promotion_converges_to_an_empty_titled_task` (un-ignored
and rewritten from the quarantined red — its doc comment keeps the history
note) plus its op-free twin
`a_bare_keyword_written_as_content_converges_to_an_empty_titled_task`, which
proves the case is a property of the BYTES and not of the promotion op.
