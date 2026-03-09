# CASES=256 soak found a pre-existing oracle bug: link-label inner-whitespace over-trim

A `PROPTEST_CASES=256` soak of the keystone (`general_e2e_composed_pbt`, `sequential
1..40`) — run to build confidence in the new shadow-mesh oracle — failed at **case 79**
on a bug that has nothing to do with the shadow mesh. Two Fable-agent consults + an
independent trace agree: **PRE-EXISTING oracle-modeling gap, high confidence.** Not a
regression from this session's peer-oracle / convergence-settle / widget-snapshot work.

## The divergence

Minimal shrunk seed (2 transitions, no peers):
```
BulkExternalAdd { doc_uri: block:structural-page,
                  blocks: [..., bulk-3-3 = "[[ ]]"] }   # or "[[ tl]]" — a link with a leading inner space
Indent { block_id: bulk-3-3 }
```
`Indent` dispatches via a chord that clicks the block first → opens its editor
(`transitions/indent.rs:97`, `model_chord_click_focus`, `transitions/mod.rs:51-77`).
Both sides open an editor on the link block and disagree on its editable text:

- `inv-editor-text-matches-ref`: ref `"tl"` vs SUT MutableText `" tl"` (leading space).
- `inv-editor-caret-matches-ref`: ref caret 2 vs SUT caret 3 (purely downstream of the
  one dropped space).

Reproduced deterministically ~10× across shrink iterations — a stable content mismatch,
NOT the flaky "SUT hasn't caught up" shape a settle-timing regression would produce.

## Root cause (ORACLE is wrong, SUT/prod is right)

- **SUT `" tl"` (correct):** the real org pipeline parses headline `* [[ tl]]` →
  outer-trim (no effect) → `extract_inline_marks` (`holon-org-format/src/inline_marks.rs:34`)
  yields link label `" tl"` — inner space **preserved**. `editor_live_text`
  (`frontend_slice/components.rs:1329`) reads that via the CDC content cell.
- **Ref `"tl"` (wrong):** the oracle seeds its editor from `block_content`
  (`reference_capabilities.rs:99`), whose value was normalized at bulk-add by
  `normalize_content_for_org_roundtrip` (`pbt/types.rs:164-193`, from
  `bulk_external_add.rs:165`). That fn runs a **fixed-point loop interleaving
  `trim`/`trim_start` with `extract_inline_marks`**, applying an extra `trim_start` to the
  post-extraction label the real parser never applies: `[[ tl]]` → label `" tl"` →
  `trim_start` → `"tl"`.

## Fix direction (when picked up — belongs in the ORACLE, not prod)

`normalize_content_for_org_roundtrip` must mirror the real parser's ORDER —
outer-headline-trim, then mark-extract, with **no post-extraction re-trim** — instead of
looping trim+extract to a fixed point that over-trims whitespace living inside a link
label. Blast radius: the fn is shared oracle infra (bulk-add + possibly other callers), so
the fix needs a full CASES=256 soak to validate it doesn't shift other seeds. Not urgent;
does not block anything at CASES=16.

## Disposition taken

Restored the green baseline `general_e2e_composed_pbt.proptest-regressions` (removed the
soak-appended `cc` line) — safe, because the bug is provably on a path this session never
touched. Failing seed preserved at `/tmp/regressions-WITH-soak-finding.txt`. Keystone green
at CASES=16 by construction.

## Meta

This VALIDATES the "longer soak + port more invariants" confidence approach (chosen over
cargo-mutants, which can't even reach `crates/holon-integration-tests/**` — it's excluded
in `mutants.toml`). The soak found a real latent oracle bug on its first outing. It also
showed a second-order benefit of this session's perf work: the faster convergence settle is
what made a 256-case soak cheap enough to run at all (~16 min), which is what reached the
`[[ x]]` extended-gen arm (`generators.rs:119-124`) that CASES=16 never sampled.
