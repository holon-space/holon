---
id: 2026-10-09-gpui-draws-every-modal-condition-as-a-quarantine
date: 2026-10-09
gap: PERCEPTION
secondary: ORACLE
status: FIXED
summary: >-
  The GPUI window drew every condition whose profile places it in a modal as a
  share-snapshot quarantine: a vault that does not sync read "Share `<vault>`
  could not be restored. Your edits before the corruption are quarantined at
  `Holon is not syncing the files in ...`".
---

## Bug
Found by the adversarial verifier of the boot-always A1+A2 lane
(`lane-logs/a12-final-verify.md`, D5), reading the placement arms of
`ShareUiState::apply_degraded` next to the Banner fallback that lane added.

## Root cause
`apply_degraded` (`frontends/gpui/src/share_ui.rs`) built a `QuarantineEvent`
for every `ConditionPlacement::Modal`, with the condition's detail headline as
the quarantine path. Two kinds carry that placement
(`crates/holon-api/src/condition_profile.rs`): `SnapshotLoadFailed`, whose
payload is the quarantine path, and `VaultSyncNotStarted`, which is no
quarantine. The only modal the window has is the quarantine modal.

## Missing piece
The unit tests of `apply_degraded` raised only `SnapshotLoadFailed` for the
Modal placement, so the arm was never judged against another Modal kind. No
windowed test raises `VaultSyncNotStarted`.

## Remedy
Only `SnapshotLoadFailed` opens the quarantine modal, with its own payload as
the path. Any other Modal kind has no surface in this window and, as ADR 0035
rules for a missing surface, is drawn as a keyed toast with its kind logged
once at WARN. Pinned by
`share_ui::tests::a_modal_condition_that_is_no_quarantine_is_drawn_as_itself`;
red in `lane-logs/a12-fix6/red-d5-modal.log`.
