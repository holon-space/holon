# orgize inline AST audit (Phase 0.3)

**orgize version**: `0.10.0-alpha.10` (alpha — confirm version doesn't drift between Phase 1 and ship).
**Audit source**: `crates/holon-org-format/examples/inline_ast_audit.rs`
**Reproduce**: `cargo run -p holon-org-format --example inline_ast_audit 2>&1 | tee /tmp/orgize-audit.log`

## Verdict

orgize 0.10.0-alpha.10 produces every inline AST node the plan needs.
Coverage matches the plan's mark vocabulary 1:1 (Bold, Italic, Underline,
Verbatim, Code, Strike, Subscript, Superscript, Link). One known limitation
documented below (backslash escapes), addressable via post-processing inside
the parser without changing the model.

## Coverage table

| Plan mark    | orgize SyntaxKind | Range basis (rowan) | Notes |
|---|---|---|---|
| Bold         | `BOLD`            | byte offsets        | Surrounds `*…*`; range covers delimiters |
| Italic       | `ITALIC`          | byte offsets        | Surrounds `/…/` |
| Underline    | `UNDERLINE`       | byte offsets        | Surrounds `_…_` |
| Verbatim     | `VERBATIM`        | byte offsets        | Surrounds `=…=` |
| Code         | `CODE`            | byte offsets        | Surrounds `~…~` |
| Strike       | `STRIKE`          | byte offsets        | Surrounds `+…+` |
| Subscript    | `SUBSCRIPT`       | byte offsets        | Range starts at `_{`, not at the preceding char |
| Superscript  | `SUPERSCRIPT`     | byte offsets        | Range starts at `^{`, not at the preceding char |
| Link         | `LINK`            | byte offsets        | One node for `[[uri][label]]` and `[[uri]]` |

All inline nodes carry `start()`, `end()`, `text_range()`, `raw()` accessors
(byte offsets via rowan `TextSize`). Phase 1 parser must convert byte→Unicode
scalar offsets at the boundary, since Loro `mark` uses scalar offsets (per
`docs/loro_marks_findings.md` decision).

## Important observations from the audit

### 1. Headline inline traversal (H18 confirmed)

Input: `* TODO *important* thing`

```
HEADLINE [0..24]
  HEADLINE_TITLE [7..24] "*important* thing"
    BOLD [7..18] "*important*"
```

The structural leading `*` is part of `HEADLINE`, not `HEADLINE_TITLE`. Inline
traversal of the title yields BOLD as expected. **The plan's "headlines are
rich" decision is safe**: walk `HEADLINE_TITLE` children, never the headline
prefix.

### 2. Nested marks come through cleanly

Input: `*bold _and italic_*`
```
BOLD [0..19]
  UNDERLINE [6..18] "_and italic_"
```

Input: `[[https://x][*bold label*]]`
```
LINK [0..27]
  BOLD [13..25] "*bold label*"
```

Both patterns descend correctly. The Phase 1 parser walks children in the
inline subtree and emits stacked marks per range — Peritext supports the
overlap natively.

### 3. Markup at word boundary (orgize enforces correctly)

Input: `a*not bold*b` — produces NO `BOLD` node. Plain text.

This is correct Org behavior (markup must be at word boundary). Our parser
inherits orgize's enforcement; no extra work needed.

### 4. Sub/Super range starts at the trigger character

Input: `a_{sub}` → `SUBSCRIPT [1..7] "_{sub}"`. The `a` before is plain text;
the Sub range begins at `_`. When we emit a `Sub` mark, the **rendered**
text in the rich model should be just `sub` (the inner content), with the
renderer adding `_{…}` wrapping.

**Phase 1 implication**: parser's inline walker for Sub/Super needs to peel
off the `_{`/`^{` prefix and `}` suffix when extracting the inner text. The
Org renderer must reproduce them on serialize.

### 5. Backslash escapes are NOT honored by orgize 0.10.0-alpha.10 ⚠️

Input: `\*not bold\*` → orgize STILL produces `BOLD [1..12] "*not bold\*"`
(treats the trailing `\*` as a literal asterisk inside the bold range).

**Workaround options for Phase 1**:

| Option | Description | Cost |
|---|---|---|
| (a) Accept | Ship as-is; document that `\*` escapes are not preserved by orgize | Free; user-visible bug |
| (b) Pre-process | Before handing to orgize, replace `\*` etc. with sentinel tokens, then restore in renderer | Medium; sentinel collisions possible |
| (c) Post-process | After orgize parsing, inspect raw paragraph text for `\<delim>` patterns and override mark spans | Medium; brittle |
| (d) Bump orgize | Watch upstream for a later alpha that fixes this | Free if it lands; uncertain timing |

**Recommendation**: (a) for Phase 1 — backslash escapes are rare in practice
and the lossy behavior is loud (the bold mark just extends one char too far).
If users complain, (c) is the cleanest fix; the parser already needs
byte-range gymnastics so adding escape detection isn't extra structure.

### 6. Empty / pathological cases handled

- `**` alone → no BOLD node (plain text). ✓
- `* *bold*` (headline whose title starts with bold) → `HEADLINE > HEADLINE_TITLE > BOLD`. ✓
- `- item _underscored_ word` → `LIST > LIST_ITEM > LIST_ITEM_CONTENT > PARAGRAPH > UNDERLINE`. List bullet doesn't interfere. ✓

## Decisions locked in

| Decision | Choice | Rationale |
|---|---|---|
| Inline AST source | orgize 0.10.0-alpha.10 (no fork) | Coverage is complete |
| Offset conversion | Byte → Unicode scalar at parser boundary | rowan emits bytes; Loro wants scalars |
| Backslash escapes | Document as known lossy in Phase 1 ship | Rare in practice; loud failure mode |
| Sub/Super inner text | Strip `_{`…`}` / `^{`…`}` at parse, re-wrap at render | Matches the plan's "rich model is neutral" principle |
| Headline rich text | Walk `HEADLINE_TITLE` children only, not the headline prefix | Structural `*` is in `HEADLINE`, not the title |

## Follow-ups (Phase 1 parser work)

- Convert rowan `TextSize` byte offsets → Unicode scalar offsets per paragraph (track running `char_count` while walking text).
- Walk inline tree with a recursive emitter that stacks marks (preserves nesting like `BOLD > UNDERLINE`).
- For `Link`, parse the `[[uri][label]]` / `[[uri]]` shape using existing `link_parser.rs` to extract `EntityRef` (External vs Internal) — do not re-implement.
- Add a parser unit test for `* TODO *important* thing` per H18, plus `\*not bold\*` to lock the lossy behavior into a regression test.
