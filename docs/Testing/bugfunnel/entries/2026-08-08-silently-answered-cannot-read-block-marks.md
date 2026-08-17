---
id: 2026-08-08-silently-answered-cannot-read-block-marks
date: 2026-08-08
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `OperationWrapper` silently answered "I cannot read block marks" for a
  provider that can, so removing a `[[link]]` in SqlOnly leaves the dead
  link's mark AND its `block_links` junction row alive.
source_line: 1183
---

## Bug

(verifier code-reading, root-causing task #23) **`OperationWrapper` silently
answered "I cannot read block marks" for a provider that can, so removing a
`[[link]]` in SqlOnly leaves the dead link's mark AND its `block_links`
junction row alive.** The wrapper forwarded
`operations`/`find_operations`/`execute_operation`/`get_last_created_id` but
not `read_block_content_marks`, taking the trait's `Ok(None)` default — and
the wrapper, not the provider it wraps, is the member the operation registry
holds in BOTH wiring arms (`crates/holon-app/src/turso_seams.rs:989,1008`).
The dispatcher therefore took its documented fail-safe branch
(`operation_dispatcher.rs:906-940`, never null marks on an unknown prior
state — BugFunnel #66), which is right as a policy and wrong as a fact.
Reproduced verbatim: content `"just plain text now"` still carrying `Link
"Gone Page" 0..9`, a stale span that can point past the end of the text it
annotates (the out-of-bounds class of the 2026-07-20 `split_block` row).
`identity_minter` was unforwarded in the same way — latent only, since
nothing calls it across a `dyn` boundary today.

## Root cause

found by a verifier READING `OperationWrapper` while root-causing task #23,
not by any test — `crates/holon-core/src/operation_wrapper.rs` forwarded
`operations`/`find_operations`/`execute_operation`/`get_last_created_id` but
NOT `read_block_content_marks`, so it took the trait's `Ok(None)` default
meaning "this authority cannot read marks". The wrapper is the member the
operation registry actually holds in BOTH wiring arms
(`crates/holon-app/src/turso_seams.rs:989,1008`), so in SqlOnly the
dispatcher asked the wrapper, got `None`, and took its documented fail-safe
branch (`operation_dispatcher.rs:906-940`, BugFunnel #66 — never null marks
on an unknown prior state). Correct as a fail-safe, wrong as a fact: the
provider underneath CAN read the row. USER-VISIBLE DEFECT: removing a
`[[link]]` from a block leaves the link's mark alive on the replacement text
and leaves its `block_links` junction row standing — a dead link that still
backlinks, and a mark span that can point past the end of the text it
annotates (the out-of-bounds class of the 2026-07-20 `split_block` row).
Reproduced verbatim: content `"just plain text now"` still carrying `Link
"Gone Page" 0..9`. NOT AN ORACLE GAP — `inv-blocks-match-ref/block_raw`
compares the Marks facet (`holon-pbt-core/src/block_compare.rs`) and would
have gone red on the very first divergent draw; the oracle was adequate and
simply never presented with the state. NOT AN ENVIRONMENT GAP EITHER — the
keystone boots the REAL DI path (`holon_app::new_from_config_with_di`,
`test_environment.rs:429`), so the wrapper is genuinely in its wiring; the
failing code path runs there. Purely generation: no draw ever minted marks
on a block and then edited THAT SAME block to mark-free text, the one
two-step sequence that separates "unreadable authority" from "readable
authority". Measured, not assumed — 128 cumulative composed-keystone cases
across two 64-case runs produced ZERO `marks: sut=… ref=…` divergences over
25 and 35 SqlOnly draws respectively. Compounding the miss: both 64-case
runs abort at roughly case 13 of 64 on an unrelated hard panic (`Cannot
outdent a direct child of a page … ADR 0028 D1`, raised at
`crates/holon-integration-tests/src/pbt/op_write_cap.rs:90`, absent from
`docs/Testing/KeystoneKnownReds.md`), which caps every full keystone run at
~20% of its cases and is filed separately)

## Missing piece

Neither the oracle nor the wiring was missing.
`inv-blocks-match-ref/block_raw` compares the Marks facet
(`holon-pbt-core/src/block_compare.rs`) and would have gone red on the first
divergent draw, and the keystone boots the REAL DI path
(`holon_app::new_from_config_with_di`, `test_environment.rs:429`) so the
wrapper genuinely sits in its provider stack. The miss was purely
generation: no draw ever minted marks on a block and then edited THAT SAME
block to mark-free text — the one two-step sequence that tells a readable
authority apart from an unreadable one. Measured: 128 cumulative
composed-keystone cases over two 64-case runs, 25 and 35 SqlOnly draws, ZERO
`marks: sut=… ref=…` divergences. Missing piece = a generator arm that
follows a mark-minting `TypeChars` with a mark-free edit of the same block.

## Remedy

FIXED 2026-08-08 (lane MARKS-FORWARD): `OperationWrapper` now forwards
`read_block_content_marks` and `identity_minter`
(`crates/holon-core/src/operation_wrapper.rs`), matching the
`OrderedBlockCrud` precedent landed at 684c061f. GAP CLOSED BY A PAIR, which
is the whole coverage argument:
`removing_a_link_clears_marks_through_the_wrapped_authority` drives the real
dispatcher through the REGISTERED wrapper and sits directly beside
`live_content_edit_removing_link_clears_junction`, which drives a BARE
provider and always passed (`crates/holon/tests/live_edit_link_marks.rs`) —
the twin proves the wrapper and not the provider was the defect. Red log:
`left: Some("[{\"start\":0,\"end\":9,\"kind\":\"Link\",…\"Gone Page\"…}]")`
vs `right: None`; plus two forwarding unit reds in `operation_wrapper.rs`,
both `left: None`. Gates: fmt clean, archlint 0 new, holon-core+holon-app
152/152, keystone-smoke 4/0 twice, hand-authored 34/34 + 9/9. The
keystone-level generator arm named above is NOT closed by this lane.
