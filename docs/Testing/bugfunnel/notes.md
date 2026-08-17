Seeded 2026-07-07 from the retroactive audit of documented dogfood/triage bugs.

| Date | Bug (one line) | Primary gap | Secondary | Missing piece | Remedy status Round-2 verifier precisions: the unconditional self_replacements.clear() gives the interleave residual blast radius N (every in-flight write-back), not 1; and NO test guards the delivery-order assumption itself (unit layer replays the recorded order; the live rung passes because this platform orders the two halves of one rename(2) correctly — 0/40 misclassifications measured at the tightest user-producible interleave). Delivery-independent hardening = task #38. |
|---|---|---|---|---|---|


Notes:
- The 2026-07-09 cold-boot-trickle escape is the boot-path sibling of the
  2026-07-05/07-08 edit-path latency escapes: same class (latency-over-budget
  that only breaches at real scale because the keystone's tiny vault hides it).
  Primary ENVIRONMENT because the failing cadence (N files × per-file feed
  barrier) needs a many-file vault that the 2-file keystone boot never creates;
  ORACLE secondary because no boot-to-pages SLO invariant would have flagged it.
- The 2026-07-05 latency bug was originally classed PERCEPTION in the audit;
  reclassified ENVIRONMENT/ORACLE on 2026-07-07 when latency-over-budget was
  declared a formalizable bug (SLO above).
- The 2026-07-08 org-writeback latency escape is the read/render-side sibling of
  the 2026-07-05 projection-side latency escape: same class (an O(N)
  full-document re-materialization per single-block edit), now on the org
  writeback path. Primary ORACLE because the interaction (a content edit) and
  the failing code path (`on_block_changed → get_blocks`) both run in the
  keystone wiring — only a per-edit SLO/recursive-CTE-count invariant is
  missing; ENVIRONMENT secondary because the recursive CTE is milliseconds at
  the keystone's handful of blocks and only breaches wall-clock at vault scale.
  The added regression test asserts the structural proxy (zero recursive-CTE
  per content edit); a keystone p95<200ms writeback invariant remains the open
  gap.
- Successes are not escapes: the editor-caret divergence was *found by* the
  keystone oracle and does not belong here. It has now recurred twice, and both
  faces are the same class — the reference's chord-click model and the SUT's
  driver deciding "no click happened" from DIFFERENT state, so the ref re-seeds
  the caret to end-of-text while the SUT leaves it alone. Face 1 was
  `SplitBlock → <chord op>` (ref end-of-text vs SUT 0); face 2 (2026-08-09,
  task #58, `inv-editor-caret/mirror … cursor_byte=<content.len()>, SUT tracked
  caret=0`, ~1 in 3 keystone-smoke runs) was triggered by #36's creation-slot
  birth, which seats focus and a caret seed at 0 WITHOUT mounting a reference
  editor — a focus-without-editor state that had never existed before, and on
  which the ref's active-editor-only guard silently stopped mirroring the SUT's
  `engine.focused_block()` guard. Fixed by making the two predicates one
  (`model_chord_click_focus` in `crates/holon-integration-tests/src/pbt/
  transitions/mod.rs`, which now cites the two `user_driver.rs` sites it must
  track); the deterministic lock is the `birth-then-chord-loses-the-caret`
  hand-authored case. Still not escapes — the keystone caught both — but any
  third face is a signal that this pair of predicates needs one owner rather
  than two comments.
- The 2026-07-06 "iOS Focus/Blur never fire → tap doesn't move `focused_block`,
  keyboard/commit dead" premise (memory `ios-text-2-causes`) was VERIFIED FIXED
  on 2026-07-09 against the live iOS sim with real `idb` finger taps: a tap moves
  the editor authority, typing lands, and moving focus away commits — the
  Petri-net/`InputRouter` rework closed it. No longer an open escape.
