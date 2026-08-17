---
id: 2026-07-22-enter-reportedly-duplicates-block-content-cursor
date: 2026-07-22
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Enter reportedly duplicates block content (cursor at line start; and
  mid-large-block). Backend split_block VERIFIED correct at op-level (pos 0 →
  empty origin + one new block with content; mid → clean partition, no dup).
  Suspected UI/EBO defect: origin EditorView InputState buffer not reconciled
  to the truncated backend content after Enter, so both origin and new block
  show the text
source_line: 1091
---

## Bug

Enter reportedly duplicates block content (cursor at line start; and
mid-large-block). Backend split_block VERIFIED correct at op-level (pos 0 →
empty origin + one new block with content; mid → clean partition, no dup).
Suspected UI/EBO defect: origin EditorView InputState buffer not reconciled
to the truncated backend content after Enter, so both origin and new block
show the text

## Missing piece

keyboard-into-editor + EditorView buffer/projection reconciliation not
exercisable headless (McpUserDriver focus wall); no assert that post-split
origin editor buffer == truncated backend content

## Remedy

open — RETESTED on EBO build (main 5e3bd9d9, debug holon-gpui) 2026-07-22
via live GPUI + dogfood-explorer MCP on a seeded `Notes` page (3
org-ingested children). Enter/split verified CORRECT on EVERY focusable
editor: (1) end-of-line → new empty sibling, content preserved; (2)
line-start (caret 0 via `home`) → origin emptied, content moved to a new
block below, NO duplication (content appears exactly once); (3) mid-block →
clean partition ("gamma"/"bullet content"); (4) empty-content block → new
empty sibling; (5) mid-LARGE-block (216-char, split at 95) → 95+121=216
exact, NO duplication. Duplication is thus NOT reproducible through
MCP-synthesized keystrokes — CONFIRMS the row-292 diagnosis that the real
trigger is an EXTERNAL write landing mid-focus (synthesized keys take the
clean split path and never hit the stale-buffer clobber). The
separately-reported "cannot CREATE a block by pressing Enter"
(creates-nothing) variant was ALSO not reproducible on any focused editor;
the only no-op cases were blocks MCP could not focus at all —
page-title/page-rows (journal date), the `block:__virtual:*` creation slot
(bounds never commit), and title-less degraded doc-roots — i.e. the
McpUserDriver focus wall this row already names, NOT a split defect.
Enter→split routes via the reactive trigger, not
`EditorViewModel::apply_local_edit` (which was clean on inspection), so the
prime suspect is exonerated. No contained fix / red-first test possible
pre-refactor (HeadlessEditorMirror bypasses EditorViewModel/InputState →
cannot go red); remedy remains the ratified EditorBufferOwnership refactor
(docs/Plans/EditorBufferOwnership-2026-07-20.md) which moves buffer+seq+echo
policy into EditorViewModel so a mid-focus-external-write rung can drive it
headless. + STATUS 2026-07-24 (lane3 enter-split repro attempt): re-verified
the composed keystone's SplitBlock transition is ALREADY
caret-position-parameterized — split_block_weighted_generator
(transitions/split_block.rs:215-243) enumerates every char_indices byte
offset PLUS text.len() (positions 0/1/mid/len-1/len all drawn),
char-boundary gated; under HOLON_PBT_EXTENDED_GEN the split content spans
2/3/4-byte codepoints (é/ß/ñ/€/中/日/😀/🎉, content_generators.rs:24-30) and
[[link]] marks (generators.rs:251). So the branch-1 coverage this lane
hypothesized as MISSING is already present in-tree; the class stays
red-impossible headless for the row's stated structural reason
(KeystrokeBlockTreeWriter→HeadlessEditorMirror dispatches split_block
straight → store-correct, bypassing
InputState/last_local_seq/evaluate_data_sync_echo). CONFIRMS the
disposition; no count change. Remedy unchanged = EBO refactor Inc 4 headless
echo rung. CORRECTION same day (lane5 staleness check): EBO Inc 0-5 had
ALREADY LANDED pre-dating this clause (echo composition runs headless via
EditorViewModel; HeadlessEditorMirror routes vm.apply_local_edit, VM owns
last_local_seq) — the class is no longer machinery-blocked; the remaining
gap is the composed keystone external-write-while-focused TRANSITION
(scenario generator), tracked as its own lane.
