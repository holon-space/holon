---
id: 2026-09-15-conditions-invariant-matches-subject-by-raw-suffix
date: 2026-09-15
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  `inv-conditions-match-ref` decided a disclosure was present with an unanchored
  `subject.ends_with(file_name)`, so a DIFFERENT file whose name merely ended with
  the pinned one satisfied the expectation and the intended file's refusal went
  undisclosed while the invariant reported a clean run.
---

## Bug

`inv-conditions-match-ref` compares the model's expected conditions against the
conditions the production `ConditionBus` is disclosing. The model pins a FILE
NAME (`keystone-recipe.cook`); the raise site carries an absolute temp-vault path
(`/tmp/vault/keystone-recipe.cook`). The comparison was a raw string suffix
(`r.subject.ends_with(suffix)`), so the expectation was satisfied by
`/tmp/vault/a-keystone-recipe.cook` — a DIFFERENT file.

Consequence: if the gate refuses a write to the intended file but the condition
raised carries another file's name, the invariant cannot tell. It reports `Ok`,
and the user was never told about the refusal — the exact silent failure this
invariant was written to catch. The same weakness sat in the spurious direction:
a governed-kind condition on a colliding sibling was accepted as expected.

Found 2026-09-15 by the fresh-context adversarial verifier of lane
`error-remedy` (Inc 2, Claim 1 "design (a)"), by a direct probe that called
`InvConditionsMatchRef::check` with hand-written `RefConditions`/`SutConditions`.
Evidence: `lane-logs/error-remedy-inc2-verify.md` (Claim 1, REFUTING FINDING) and
`lane-logs/v2-suffix-falsepass-probe.log`. The probe was a scratch test and was
removed; the permanent regression test below replaced it.

## Root cause

Three sites shared one unanchored predicate:

- `crates/holon-integration-tests/src/pbt/invariants/bodies/conditions_match_ref.rs`
  — the `missing` direction and the `spurious` direction
- `crates/holon-integration-tests/src/pbt/conditions_state.rs` —
  `ExpectedCondition::matches`, the model's own rule

`ends_with` has no path-component boundary, so any subject whose final component
merely ENDS WITH the pinned name matched (`a-keystone-recipe.cook` for
`keystone-recipe.cook`; `recipe.cook` for `cook`). The model's doc comments
stated the suffix rule as a deliberate design ("the model pins the file name and
the invariant honours that"), which is what made the approximation look
intentional rather than weak.

## Missing piece

**ORACLE (primary).** The interaction was generatable and the invariant ran — but
the oracle's own comparison rule was too weak to flag the defect. If a case had
hit this state, `inv-conditions-match-ref` would have returned `Ok`. The sibling
`inv-read-only-home-refuses-writes` could not cover it either: it only asks
whether SOME disclosure was raised, and a colliding sibling's condition satisfies
that question.

**COVERAGE (secondary).** The keystone fixture seeds exactly one recipe file
(`READ_ONLY_RECIPE_FILE`), so no draw produces a name-colliding sibling. Even a
correct oracle would not have been exercised by the random keystone.

## Remedy

One shared predicate now owns the rule —
`holon_pbt_core::capabilities::subject_matches_expected` — and all three sites
route through it: a subject satisfies an expectation only when its FINAL PATH
COMPONENT equals the pinned name (a bare-name subject still matches whole). The
`subject_suffix` field/params were renamed to `subject_name`, and every doc that
asserted the suffix rule was corrected.

Permanent regression test: `conditions_match_ref.rs`
`#[cfg(test)] mod tests` (4 tests) — the collision must NOT satisfy the
expectation in either direction, with the pinned file itself as the positive
control.

- Red (pre-fix): `lane-logs/suffixfix-red-1789495634.log` —
  `4 tests run: 1 passed, 3 failed`; the false pass reproduces as
  `` `/tmp/vault/a-keystone-recipe.cook` merely ENDS WITH the pinned
  `keystone-recipe.cook` ``.
- Green (post-fix): `lane-logs/suffixfix-green-1789495743.log` —
  `4 tests run: 4 passed`.
- Teeth (fix reverted, same test): `lane-logs/suffixfix-teeth-1789495776.log` —
  `4 tests run: 1 passed, 3 failed`; sources restored byte-identical.

Remaining known gap: the keystone fixture still produces no name-colliding
sibling, so the composed random draw does not exercise this itself. Closing that
would mean a fixture axis that seeds a sibling whose name ends with the pinned
one; the unit regression test above is the pin that exists today.
