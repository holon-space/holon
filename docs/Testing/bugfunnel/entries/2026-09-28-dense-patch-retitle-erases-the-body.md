---
id: 2026-09-28-dense-patch-retitle-erases-the-body
date: 2026-09-28
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  dense_patch retitling a row whose block has body lines writes the new
  title as the WHOLE content, so every body line is erased and the patch
  reports success.
---

## Bug
Found by the Inc 6 round-3 adversarial verifier (`lane-logs/inc6r3v-verify.md`,
F1, probe `[zzp10]`), and present on main `d4f426ffc965`. Store one block with
`content = "Head\nbody line\nsecond body line"`, `dense_query` it, change
`Head` to `New head`, `dense_patch`: the store holds `content = "New head"`.

## Root cause
The planner diffs only the first content line: `plan_patch` compares
`db.block.org_title()` with `rec.title` (the stored first line) and emits
`UpdateTitle { title }` (main `frontends/mcp/src/dense_patch.rs:278-283`).
The applier writes that title into the whole `content` column:
`set_field(block, "content", title)` (main `frontends/mcp/src/tools.rs:792-800`).
`content` holds title AND body, so the body is replaced.

## Missing piece
The planner oracle judges "applied exactly" against a hand-written model of the
store (`apply()` in `frontends/mcp/tests/dense_patch_exact.rs`) that replaces
only the first line, so it calls the erasing edit exact. No real-engine rung
(`crates/holon-integration-tests/tests/dense_patch_tags_properties.rs`) and no
keystone transition edits a stored row that has a body.

## Remedy
Inc 6 round 4. A row edit writes the whole content: the stored title line is
kept when the headline is unchanged, the stored body when the body is
unchanged (`edited_content` in `frontends/mcp/src/dense_patch.rs`), and
`PatchOp::SetContent` replaces `UpdateTitle`. The hand model is deleted; the
exactness oracle reads the real store after the real tool
(`crates/holon-integration-tests/tests/dense_patch_engine_exact.rs`,
`a_retitle_keeps_the_body` red at base in `lane-logs/inc6r4-red.log`), and
`dense_patch` reads every written row back and rolls the patch back when the
store does not show it as written.
