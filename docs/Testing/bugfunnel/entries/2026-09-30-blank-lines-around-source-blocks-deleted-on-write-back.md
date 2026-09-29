---
id: 2026-09-30-blank-lines-around-source-blocks-deleted-on-write-back
date: 2026-09-30
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  An unedited org file lost the blank lines before a source block and at the
  start of the file, gained a blank line before preamble text, had
  `:COLLAPSED: true` rewritten to `t`, and had text after a source block moved
  before it, all with no loss: 70 of 900 files in the vault's hidden agent
  copies changed bytes on write-back.
---

## Bug
Found by the org-faithful group B r10 verifier (`lane-logs/groupB-r10-verify.md`,
D4), running write-back over the vault's hidden directories, which the lane
never measured: 66 files lost 1–6 blank lines, 2 changed only in order or
spacing, 2 gained minted ids. The hidden files are the vault's
`.claude/worktrees/agent-*/` copies (900 of the 1050 `.org` files). Holon
never ingests them: `VaultFilter` refuses a hidden path segment
(`crates/holon-filesystem/src/vault_filter.rs`, `vault_path.rs`
`hidden_vault_segment`), and the boot walk skips hidden entries. Their shapes
are valid org all the same, and the visible vault can hold them.

## Root cause
The parser dropped blank lines it had no carrier for: before a source block
(a paragraph's trailing blank lines are trimmed off the body, and the source
block is written right after it), and at the start of a file (org reads them
as part of no element). `render_pass` wrote one blank line before preamble
text whenever the page had no keyword line. `:COLLAPSED: true` was compared
as `true` against the `t` Holon writes, so the unedited drawer was
re-rendered. Text after a source block is joined to the body, which the
renderer writes before its source blocks.

## Missing piece
The write-back stability check ran over the visible vault only
(`lane-logs/B10-vault.log`), and the keystone's render fixed point renders
generated stores, never authored files.

## Remedy
`BlankLines.before_body` of a source block holds the blank lines before it,
and the page's the blank lines that start the file; `render_walk` writes the
page's section-end blank lines after its source blocks. The generated blank
line before preamble text only follows a generated header.
`DrawerReading::values` compares a boolean drawer value as Holon writes it.
Text after a source block is disclosed (`_text_after_source`, a loss on
render). The 2 files with minted ids are by design
(`ParseResult::headlines_needing_ids`, stamped by the sync controller).
Vault, visible and hidden: 0 silent changes, 0 disclosed, 3 stamped
(`lane-logs/B11-vault.log`). Pinned by `unedited_file_keeps_its_bytes.rs`.
A source block's `#+begin_src` spelling and a missing `:id` are kept
(`_source_lines`, `crates/holon-org-format/src/models.rs` `SourceLines`); an
id is written only when the block's place changes, with a loss. Pinned by
`source_block_lines_keep_their_spelling`,
`a_source_block_with_no_id_keeps_its_bytes` and
`a_moved_source_block_with_no_id_has_its_id_written`.
