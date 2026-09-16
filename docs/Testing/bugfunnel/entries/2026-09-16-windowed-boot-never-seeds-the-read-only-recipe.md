---
id: 2026-09-16-windowed-boot-never-seeds-the-read-only-recipe
date: 2026-09-16
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  The windowed composed keystone's boot seeds no `.cook` file, so its oracle
  declares `block:keystone-recipe.cook::b::0` read-only-homed while the SUT has
  no `block_raw` row for it and the harness panics mid-case.
---

## Bug

`holon-gpui::gpui_composed_windowed_loop general_e2e_composed_pbt_windowed`
(`frontends/gpui/tests/gpui_composed_windowed_loop.rs:197`) panicked at
`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:7095`:

```
    [read-only homes] block:keystone-recipe.cook::b::0 is declared
    read-only-homed but has no block_raw row to re-write
```

Capture: `/tmp/holon-land-w14-1789580360/land-w14-nextest-1789584904.log` — 5
panic sites (lines 4387, 5003, 5211, 5319, 5641) plus two restatements (4390 the
pinned first-divergence signature, 5645 the `[4b-loop]` shrunk verdict). The
minimal drawn case, at line 5212, is ONE transition:

```
    [4b-loop] case: 1 transition(s) drawn: ["AttemptIngestCompoundOnReadOnly"]
```

`attempt_ingest_compound` reads the block's own stored source
(`SELECT content FROM block_raw WHERE id = …`) so the compound re-writes what
the file already said, and panics when the row is absent. The row is absent.

Not reproduced isolated. In isolation the same windowed loop reds earlier on the
registered caret divergence (`op_write_cap.rs:381:17`, TRIAGE row 8 / the
`windowed-splitblock-no-editable-surface` row), which aborts the loop before a
case reaches the ingest transition; the full parallel gate run reaches it. The
`find that reproduced` question is therefore answered — the seed is missing
either way, and the isolated leg simply cannot get far enough to say so.

## Root cause

The windowed boot path and the headless boot path disagree about the vault they
seed. Both build their file list from the oracle, but only the headless one
knows about the recipe:

- **Headless** — `boot_and_seed_wide_with_peer_id`
  (`crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs:1065`) pushes the
  recipe at :1148, keyed on the oracle carrying read-only homes
  (`if !ref_state.read_only.homes().is_empty() { seed_files.push((READ_ONLY_RECIPE_FILE, KEYSTONE_RECIPE_COOK)); }`),
  and then WAITS for the steps to appear in `block_raw` at :1235-1265 with a
  fail-loud 10 s guard ("a recipe that never ingests means the fixture, not the
  gate, is what this run measured").
- **Windowed** — `boot_and_seed_wide_windowed_base`
  (`frontends/gpui/tests/pbt_harness/windowed_wide.rs:163` →
  `wide_e2e.rs:1429`) seeds `structural-page.org` and, conditionally,
  `forward-edge-page.org` — and nothing else. `wide_seed_tree()` (`:887`) is
  four blocks and no file. Its START barrier (`:1505`) waits on the forward-edge
  ids and the boot journal, never on the read-only homes.

The oracle is identical on both arms, and that is what makes it a panic rather
than a divergence: `wide_e2e_ref_for` calls `seed_read_only_recipe` for every
`ViewModel` (frontend) draw (`wide_e2e.rs:1766`), which inserts the two step
blocks and `state.read_only.seed_home(uri)` for each. The `RefReadOnlyHomes` cap
then gates `AttemptIngestCompoundOnReadOnly` into the windowed alphabet, so the
generator aims at a block the windowed SUT was never given.

The surrounding code already documents this exact hazard class for a different
file: the forward-edge corpus comment at `:1446` says the windowed base "MUST
ingest the matching `forward-edge-page.org` file or the SUT is permanently
missing `fe-parent`/`fe-blocked`/`fe-target`". The read-only recipe was added to
the headless path after that comment was written and the windowed base was not
updated with it.

## Missing piece

Seed parity between the two boot paths. The keystone's windowed arm carries an
oracle feature (read-only homes) for which its SUT has no backing file, so a
whole transition is gated into the alphabet against a store that cannot satisfy
it. No gate compares the two seed lists, and the windowed START barrier does not
include the read-only homes, so a missing seed surfaces as a panic inside a
drawn case instead of a loud red at boot.

## Remedy

OPEN. Two changes, both in the harness, neither in this lane (a docs/triage
lane touches no code):

1. Mirror the headless seed into `boot_and_seed_wide_windowed_base` — push
   `(READ_ONLY_RECIPE_FILE, KEYSTONE_RECIPE_COOK)` on the same condition
   (`!ref_state.read_only.homes().is_empty()`).
2. Add the read-only homes to the windowed START barrier's `start_expected`, so
   a boot that did not ingest them fails at the barrier with the guard's
   message rather than panicking later inside a transition. That is the change
   that makes this class of divergence self-reporting instead of case-order
   dependent.

The registration is ENVIRONMENT because the failing code path — a store whose
owning document is homed read-only — is exactly what the headless arm runs, and
the two arms differ only in what they seed. Making them seed the same vault is
the parity work the gap names.
