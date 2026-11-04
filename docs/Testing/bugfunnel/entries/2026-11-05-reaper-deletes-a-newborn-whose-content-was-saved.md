---
id: 2026-11-05-reaper-deletes-a-newborn-whose-content-was-saved
date: 2026-11-05
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  On a focus change the untouched-newborn reaper deletes a creation-slot newborn
  whose content set_field already saved, because no live watcher holds its row
  and the headless editor never retires the newborn.
---

## Bug
Found by a keystone replay (`general_e2e_composed_pbt`, seed
8431877175961110154, shrinking off), red 3 of 3. At `FocusEditableText` the
reaper sent `block.delete` for a newborn that a creation-slot gesture had made
and whose keystrokes had already been saved by `set_field`. The file sync then
restored the block from its copy in Journals.org under `block:journals`, so the
oracle's expected `block-in-two-files` disclosure was not in effect.

## Root cause
Measured with a log probe in `reap_untouched_newborns`
(`crates/holon-frontend/src/reactive.rs:3326`): the candidate had
`observed=None`. `observed_content` (`reactive.rs:3351`) only reads rows of
live watchers; after navigation no watcher held the newborn's row, so the
ephemeral-newborn set was the only proof and licensed the delete. The set is
retired by the keystroke sink, which the headless editor mirror never wires
(`crates/holon-frontend/src/headless_editor_mirror.rs`, no
`set_ephemeral_newborns`; GPUI wires it in
`frontends/gpui/src/views/editor_view.rs`). Same defect as
`2026-10-01-reaper-deletes-agent-work-on-an-empty-newborn`, now reproduced for
saved content.

## Missing piece
The headless frontend does not retire a newborn on a keystroke as GPUI does,
and no test pins that a reap with no observed row reads the stored content
before it deletes.

## Remedy
OPEN. Proposed known-red row and card text go to the steward; the defect is not
fixed in the lane that found it.
