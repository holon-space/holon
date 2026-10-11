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
`top_level_conjuncts` (crates/holon-profiles/src/lib.rs:448) scans bytes,
tracking literal state and parenthesis depth, and splits only on a depth-0
`&&` outside a literal. All three Rhai delimiters are tracked: `"` and `'`
process `\` escapes, a backtick literal is verbatim (`\` is content, the
literal may span lines). An unbalanced parenthesis or an unterminated literal
is refused naming the source condition instead of being split wrongly.

Pinned by thirteen unit tests in `crates/holon-profiles/src/lib.rs`
(`a_double_quoted_ampersand_pair_...` through
`an_unterminated_backtick_literal_is_refused_naming_the_condition`); red log
at `docs/Testing/red-logs/2026-10-11-split-condition-quotes-parens-RED.txt`.

The first fix attempt tracked only `"` and `'`, so `x == \`a"b\` && is_focused`
— valid Rhai — became a hard refusal. A fresh verifier caught that before the
lane landed (`target/split-cond-verify.md`); the backtick tests and the
verbatim rule close it, and the one test that did not discriminate against the
old splitter was reworked to do so.
