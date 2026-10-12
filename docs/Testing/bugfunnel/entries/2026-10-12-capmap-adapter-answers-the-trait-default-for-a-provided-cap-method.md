---
id: 2026-10-12-capmap-adapter-answers-the-trait-default-for-a-provided-cap-method
date: 2026-10-12
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  An invariant that read a default-bodied (provided) `&self` cap method through
  the `CapMap` got the trait DEFAULT instead of the registered provider's
  answer, so the read could not disagree with the model and the invariant could
  not go red.
---

## Bug
Found by code audit in lane `capmap-forward` (steward brief
`brief-capmap-forward.md`), prompted by lane `oracle-fix`, which had made
`editor_surface_text` a REQUIRED cap method for this very reason.

`#[holon_macros::capmap_adapter]` generates `impl <Cap> for CapMap`, which
forwards each cap method to `self.expect::<dyn Cap>()`. It skipped every method
that carried a default body, so the generated impl INHERITED the trait default
for those. Every invariant reads its caps off the `CapMap`, so such a read
never reached the provider.

Pinned by `crates/holon-pbt-core/tests/capmap_forwards_provided.rs`; red log
`docs/Testing/red-logs/2026-10-12-capmap-skips-provided-methods-RED.txt`
(`CapMap answered the trait DEFAULT (false) for a provided method; the
registered provider says true`), both for a sync and an `async` provided
method.

## Root cause
`crates/holon-macros/src/capmap.rs` skipped a method when
`method.default.is_some()`, with the comment "Provided (default-bodied) methods
are inherited by the impl". True for an ordinary impl, wrong for this one: the
`CapMap` impl is a FORWARDER, so inheriting a default answers from the empty
adapter rather than from the provider. The defaults in question are
"substrates without this concept answer `false`" fallbacks
(`RefBlockTree::is_source_block`, `is_rule_machinery`, `is_collapsed`,
`is_copied`, `copy_file_count`, `RefEditorMirror::active_editor_dirty`,
`SutLayout::rendered_elements_fresh`), so the faked answer was always the
can't-fail one.

Scope measured over the 109 `capmap_adapter` traits: 7 provided `&self`
methods, 0 provided `&mut self` methods.

## Missing piece
No test asserted the adapter's forwarding contract, so the skip read as a
deliberate design choice. The only defence was a hand-written convention
comment at `crates/holon-pbt-core/src/capabilities.rs:304` telling cap authors
not to write a default — one trait documented it, the other 108 did not.

## Remedy
The macro forwards provided `&self` methods exactly like required ones, and
`has_async` counts provided async methods so a trait whose only async method is
provided still gets `#[async_trait(?Send)]`. Provided `&mut self` methods stay
skipped: forwarding them through the shared `Arc<dyn Cap>` is unsound (no
mutable access), and there are none today.
