---
id: 2026-10-11-split-condition-splits-inside-quotes-and-parens
date: 2026-10-11
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A profile variant condition was split on every `&&`, including one inside a
  string literal or inside parentheses, so the frontend got a silently wrong UI
  predicate or the variant was refused for a reason that was not its fault.
---

## Bug
Found by code research during a review of the profile variant condition
splitter (lane split-cond), not by a test. `split_condition` cut the condition
on the literal text `&&` wherever it appeared. Three shapes broke:

- `view_mode == "list && table"` became the two fragments `view_mode == "list`
  and `table"`. The first parsed as a UI `Eq` whose value was the unbalanced
  `"list`, the second went data-side and failed to compile — a loud error that
  named a fragment nobody wrote.
- `view_mode == "a\" && b"` broke the same way although the `"` was escaped.
- `(is_focused && is_expanded) || is_task` has NO top-level `&&`, so the whole
  expression belongs data-side. It was cut into `(is_focused` and
  `is_expanded) || is_task`; the first fragment references only UI variables,
  so it reached `parse_conjunct_to_predicate` and was refused as an
  "unsupported UI condition" — the variant was dropped over a well-formed
  condition.

No shipped condition carries a `&&` inside quotes or parentheses, so prod was
not hit; an author-written condition in a type YAML or an org-embedded profile
would be.

## Root cause
`crates/holon-profiles/src/lib.rs` split with `source.split("&&")` — a plain
text scan with no notion of string literals or nesting. The two sibling
scanners in the same file (`has_non_ui_references`,
`find_comparison_operator`) already skip quoted spans with escapes, so the
splitter was the one quote-blind step in the chain.

## Missing piece
No test fed `split_condition` a condition whose `&&` was not a top-level
conjunct boundary: the existing cases were all flat `a && b` forms, and the
keystone PBT authors no profile conditions at all, so no generated interaction
can reach this state.

## Remedy
`top_level_conjuncts` (crates/holon-profiles/src/lib.rs:444) scans bytes
following the Rhai 1.26 tokenizer and splits only on a `&&` outside every
literal and every `()`, `[]` or `{}`:

- `"` and `'` literals process `\` escapes; raw strings `#"…"#` end at `"`
  plus the same number of `#`.
- A backtick literal is verbatim text, except that a doubled backtick is an
  escaped backtick and `${` opens an interpolation. The interpolation is an
  expression with its own nested literals and brackets; its `}` resumes the
  literal's text.
- Refused naming the source condition: an unterminated literal, an unbalanced
  or mismatched bracket, any comment (`//`, `/*`), and a split beside a
  top-level `||`, `|`, `^`, `&` or `;`. `a && b || c` is `(a && b) || c`, so
  a split on the `&&` would change the meaning.

Pinned by the unit tests in `crates/holon-profiles/src/lib.rs` from
`a_double_quoted_ampersand_pair_is_not_a_conjunct_boundary` through
`a_raw_string_literal_keeps_its_ampersand_pair`. Red logs:
`docs/Testing/red-logs/2026-10-11-split-condition-quotes-parens-RED.txt` and
`docs/Testing/red-logs/2026-10-11-split-condition-interpolation-precedence-RED.txt`.
`every_shipped_profile_condition_splits` runs every condition in
`assets/default/types/*.yaml` through the splitter, because
`profile_from_type_def` panics on a refusal.

Fresh verifiers refuted two earlier attempts before the lane landed:
- Tracking only `"` and `'` made the valid `x == \`a"b\` && is_focused` a
  hard refusal (`target/split-cond-verify.md`).
- Treating a backtick literal as flat text closed it at a backtick nested in
  `${…}`, so `x == \`${ \`&&\` }\` && is_focused` silently rewrote the inner
  literal to ` && ` (`target/split-cond-verify-r2.md`). The same review found
  the `||` precedence change, which the old `split("&&")` splitter also had.
