# Verify — pair-conflict-badge (D93.a)

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/pair-conflict-badge`
Identity sentinel: `grep -q pairing_conflict assets/default/types/block_profile.yaml` -> TREE-OK.
All logs under `/private/tmp/.../scratchpad/badge-verify-logs/`.
No jj/git write command was run. Tree left byte-identical (yaml sha256
`4f00acff71db14e2a441ddda82fc7f4bd22ce48b924a3c652241227999ffc080` before and
after; scratch test deleted, `jj status` shows the lane's 7 paths only).

## Claim 1 — windowed test paints the badge and is red for the right reason: CONFIRMED

- Green, reproduced myself: `v-gpui-green.log:4,6`
  `test a_pairing_conflict_copy_paints_its_badge ... ok` /
  `test result: ok. 1 passed; 0 failed ... finished in 24.87s`.
- Break-the-variant probe: copied the yaml aside, deleted ONLY the
  `- name: pairing_conflict` entry (`python3` slice, 527 bytes; sha256 moved
  `4f00acff…` -> `f79c532d…`), re-ran the same test:
  `v-gpui-broken-variant.log:11` `test result: FAILED. 0 passed; 1 failed`,
  `:197-198` `left: 0 / right: 1`, with the two precondition assertions
  passing above it (page rendered, 3 `rendered_text`). This is verbatim the
  red the lane report quotes.
- Restored from the copy; sha256 back to `4f00acff…` (`yaml-sha-before.txt`).
- TOOLCHAINS: I used `export TOOLCHAINS=com.apple.dt.toolchain.MetalToolchain`
  as instructed. **The environment claim does NOT reproduce.** Both
  `xcrun -sdk macosx metal --version` and a real shader compile
  (`xcrun -sdk macosx metal -c probe.metal -o probe.air`, the exact form
  `gpui_macos/build.rs:132-141` uses) succeed with `env -u TOOLCHAINS`,
  rc=0, .air produced (`metal-noTC.log` empty). I could not force a cold
  `gpui_macos` build-script rerun without invalidating the shared cargo cache,
  so this is a mechanism-level refutation, not a full cold build.

## Claim 2 — keyed on the PROPERTY, not the id suffix or the title: CONFIRMED

Static evidence:
- yaml condition is `is_def_var("pairing_conflict_of") && pairing_conflict_of != () && !is_focused`.
- `crates/holon-loro/src/device_pairing_op.rs:255-262` builds the query from
  `CONFLICT_OF_PROPERTY` via `json_extract(properties, '$.pairing_conflict_of')`;
  `conflict_copy_id` (`:266-267`, the `-before-pairing` derivation) is used only
  on the WRITE side, never in the query or the variant.

Dynamic evidence (scratch windowed test written, run twice, then deleted):
seeded page carrying (a) a block with the property on an id sharing nothing
with the derived shape (`zz-nothing-like-a-copy`) and (b) a decoy whose id ends
`-before-pairing` and carries no property.
- Property present: `v-keyprobe3-withprop.log:5`
  `PROBE badges_unfocused=["kept from this device before pairing"]` — 1 badge,
  both blocks on screen (`:4`).
- Same page, property line removed, decoy id unchanged:
  `v-keyprobe4-noprop.log:5` `PROBE badges_unfocused=[]` — 0 badges.
Property alone drives it; the `-before-pairing` suffix and the title do not.

## Claim 3 — precedence and no collateral change: CONFIRMED (with one untested tie, see GAPS)

- Focused conflict copy reaches `editing`: `v-keyprobe3-withprop.log:6-7`
  `PROBE badges_focused=[]`, `PROBE editable_before=0 editable_after=1` after
  `apply_focus_editable_text` on the conflict copy. The badge disappears and an
  editor opens, i.e. `!is_focused` does what it claims.
- `just keystone-smoke` run by me: `v-fmt-smoke.log`
  `test result: ok. 4 passed; 0 failed ... finished in 66.34s`, engagement line
  `inv-no-declared-column-absent=19/19` (fully selected, all passed; the lane's
  "31/31" is the same invariant at a different case count).
- `EXPECTED_GAPS` in
  `crates/holon-integration-tests/tests/span_capture_suite/declared_column_parity.rs:110-118`
  is untouched and does not mention the property — consistent with "not a
  declared column".
- `tracked_layout_neutrality` 1 passed, `layout_insta` 4 passed (`v-rest.log`),
  so the badge's new tracker is layout-transparent and the single-line snapshot
  change is the whole blast radius.

## Claim 4 — disclosure count + query link: CONFIRMED for what it asserts; the count->UI seam is a GAP

- Headless test passes when I run it: `v-rest.log`
  `PASS [13.571s] ... production_pairing_discloses_conflict_copies_and_how_to_find_them`,
  `Summary [13.573s] 1 test run: 1 passed, 31 skipped`.
- The stated headless limitation is REAL.
  `crates/holon-integration-tests/src/pbt/composed/two_instance.rs:738-746`:
  `boot_two_instances_with_an_empty_receiver_on` delegates to
  `boot_two_instances_seeded_on(..., &[])` — the receiver's seed list is empty
  by construction, so no id can diverge; the lane's own red output shows
  `reimported_blocks: 0, conflict_copies: 0`.
- The windowed test does NOT cover a non-zero count: it seeds the
  `pairing_conflict_of` property directly in org and never invokes the pairing
  op. So no tier asserts a count >= 1 reaching the UI (see GAPS).

## Claim 5 — no title mutation: CONFIRMED

`jj diff -r @ --git | grep -Ei '^\+.*(conflicted copy|conflict copy|set_content|title|content = format|content.push)'`
returns only comment/doc/assert-message lines (diff lines 11, 75, 206, 212,
263-264, 388, 397). No added line writes to a block's content/title. The org
write-back tripwire is untouched.

## Claim 6 — fmt and crate gate: CONFIRMED

- `cargo fmt --check` exit 0 (`v-fmt-smoke.log:2` `FMT_OK`).
- `cargo nextest run --no-fail-fast -p holon-frontend -p holon-loro -p holon-app`:
  `v-rest.log` `Summary [111.424s] 1116 tests run: 1116 passed (1 slow), 4 skipped`.
  The failure set is EMPTY — trivially a subset of the known list. No iroh
  `connection lost` failure occurred, so no isolation replay was needed.

---

# DEFECTS

**D1 — the disclosed query is truncated out of the toast for any realistic
archive path.** `frontends/gpui/src/share_ui.rs:1721` sets
`MAX_DETAIL_CHARS = 320` and `:1729-1731` cuts `detail` there. The lane appends
`Find the copies with: <query>` at the END of a detail that already embeds the
absolute archive path (`crates/holon-loro/src/device_pairing_op.rs:885`,
`archive.display().to_string()`, built from `store_dir.join("archive")` at
`:1061`). Measured: the detail's fixed part is 280 chars, the query is 97, so
the query survives intact only when the archive path is <= 40 chars. With a
realistic path
(`/Users/martin/Library/Application Support/holon/loro-store/archive/20260908-011500`,
82 chars) the detail is 363 chars and the toast shows
`... Find the copies with: SELECT id, content FROM block WHERE json_extract(prope…`
— an unrunnable fragment. Nothing tests the rendered toast string.
(Note: the lane report states the cap is 80 chars; the code says 320. The
report's number is wrong, the problem is real either way.)

# GAPS

**G1 — no tier asserts a conflict count >= 1 reaching the UI.** Headless cannot
produce one (empty receiver, evidence above); the windowed test bypasses the
pairing op by seeding the property in org. The real seam — a pairing run that
writes N copies, then a badge on those N blocks and an N in the disclosure — is
covered by nothing. The unit tests at
`crates/holon-loro/src/device_pairing_op.rs:1181-1201` prove `plan_reimport`
produces a divergent copy with the property, but stop at the plan.

**G2 — the headless disclosure assertion cannot fail on a wrong count.** It
asserts only `payload["conflict_copies"].as_u64().is_some()` — a hardcoded 0
would pass. Only the query string is asserted for content.

**G3 — the `share_ui.rs` toast change has no test at all.** No test reads the
`PairingReimportedLocalContent` detail string; D1 above is the consequence.

**G4 — the priority-0 tie with `rule_card` is untested and file-order
dependent.** Both variants sit at priority 0; `rule_card` is written first in
`assets/default/types/block_profile.yaml`. A conflict copy of a rule head (or
any block matching `is_program`) therefore renders as the rule card with NO
badge — the silent-merge case the feature exists to prevent. Same for a
`page_title` (priority 2) conflict copy. Nothing pins this ordering.

**G5 — `badge` elements record no `entity_id`.** `badge.rs` passes
`node.row_id()`, which is `None` for the badge node; measured in my probe
(`PROBE badges_unfocused=[("", "kept from this device before pairing"), ...]`),
and the nearest tracked ancestor carrying an entity resolves to
`block:default-main-panel`. So no test can assert WHICH block wears a badge from
the registry — the lane's own test can only count badges page-wide. I had to
disambiguate by running the page twice with and without the property.

**G6 — `dogfood-explorer` has not run** (lane discloses this; the
`holon-feature` skill's final gate is still owed).
