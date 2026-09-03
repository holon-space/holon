# Verify rev 2 — pair-conflict-badge (D93.a)

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/pair-conflict-badge`
Identity: `grep -q PairingReimported frontends/gpui/src/share_ui.rs` -> ID1-OK;
`assets/default/types/block_profile.yaml:154` `priority: 3` (the `pairing_conflict`
variant, `:153`). Logs: `/private/tmp/.../scratchpad/badge-verify-r2-logs/`.
No jj/git write ran. All 4 probe-edited files restored byte-identical
(`sha-before.txt`, `sha-ti.txt` vs the final `shasum` — `RESTORED-IDENTICAL`,
`TI-RESTORED`); `jj status` shows the lane's 9 paths only, no scratch file.

## Claim 1 — toast paints the whole query on its own line: CONFIRMED

- Lane test green under me: `s3-scratch.log:31`
  `test share_ui::tests::the_rendered_pairing_toast_carries_the_whole_query ... ok`
  (`:39` `27 passed; 0 failed`).
- Scratch variant (added to the private `mod tests`, run, then restored):
  - 82-char archive `s3-scratch.log:20-22`: `lines=2`, `l0_len=277` (uncut, ends
    with the full archive path), `l1=Find the copies with: SELECT id, content
    FROM block WHERE json_extract(properties, '$.pairing_conflict_of') IS NOT NULL`.
  - 192-char archive `:23-25`: `l0_len=356` — the message IS cut (`…` after the
    padded path) yet still renders the detail's opening clause, and `l1` is the
    same COMPLETE query. So the cap can never touch the query.
  I asserted `lines.len()==2`, `lines[1].contains(query)`, and
  `lines[0].contains("block(s) written on this device")` for both.
- Other kinds unchanged: `toast_lines` (diff `diff-share_ui.patch:+1749..+1758`)
  pushes the extra line only under `kind == DegradedKind::PairingReimported`;
  every other kind returns `vec![toast_message(toast)]`. Measured for a
  non-pairing kind: `s3-scratch.log:26`
  `SCRATCH other kind=SqlProjectionFailed lines=["⚠  Shared edit not shown — boom"]`,
  asserted `== vec![toast_message(t)]`. The 26 other `share_ui` unit tests
  (incl. `no_detail_length_can_panic_the_render`) pass. `degraded_bus_bridge_windowed`
  is not a test in this tree (`cargo nextest`/`cargo test` name does not exist);
  the equivalent coverage run is the full `share_ui` unit set above plus the
  crate gate (claim 5).

## Claim 2 — headless count >= 1 is real and falsifiable: CONFIRMED

- The seed IS a divergence: receiver seed `two_instance.rs:753-754`
  `#+ID: structural-page\n* the words this device wrote\n:PROPERTIES:\n:ID: parent\n:END:\n`
  vs the owner's `WIDE_TREE_ORG`
  (`crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs:247-249`)
  which holds the SAME id `parent` with content `parent`. Same id, different text,
  before pairing.
- Assertion is `assert_eq!(payload.get("conflict_copies").and_then(as_u64), Some(1), ...)`
  (`crates/holon-integration-tests/tests/two_instance_composed_pbt.rs:1602-1608`) —
  an equality, not `is_some()`.
- Baseline green, reproduced by me: `s4-base.log:188`
  `PASS [26.998s] (1/1) ... production_pairing_discloses_conflict_copies_and_how_to_find_them`,
  `:190 Summary 1 test run: 1 passed, 31 skipped`.
- Mutation probe (seed text changed to `parent`, i.e. IDENTICAL to the owner's;
  cp aside, restored by sha256): `s4-mutated.log:221-223`
  `assertion left == right failed ... got {"containers":1,"reimported_blocks":0,
  "conflict_copies":0,"conflict_query":"SELECT id, content FROM block WHERE
  json_extract(properties, '$.pairing_conflict_of') IS NOT NULL"}`,
  `left: Some(0) / right: Some(1)`, `:227 Summary 1 test run: 0 passed, 1 failed`.
  The count moves with the divergence; a hardcoded 0 cannot pass.
- The "carrying `pairing_conflict_of`" half is INDIRECT: the payload's count is
  `reimported.divergent.len()` (`device_pairing_op.rs:1105-1108`) and the query
  is only asserted to CONTAIN `CONFLICT_OF_PROPERTY`. No assertion in this test
  reads a written block's properties (see GAP G2').

## Claim 3 — priority 3 beats everything, focus still reaches `editing`: CONFIRMED

- Priorities enumerated from `assets/default/types/block_profile.yaml`:
  `pairing_conflict` 3 (`:154`); `page_title` 2 (`:72`);
  `embedded_page_expanded` 2 (`:90`); `embedded_page` 1 (`:109`);
  `rule_card` 0 (`:132`); all nine remaining variants (`source_editing`,
  `holon_source`, `query_block`, `query_block_titled`, `image_block`, `editing`,
  `source`, `default`) at -1 (`:159,163,167,174,178,182,186,190`). 3 is the
  unique maximum.
- The `embedded_page` collision pin is green under me: `s1.log:6-9`
  `test a_conflict_copy_that_is_also_a_page_still_wears_its_badge ... ok`,
  `2 passed; 0 failed ... 12.29s`.
- Rev-1 claim-3 probe repeated (scratch third test in the windowed file, then
  restored): with `apply_focus_editable_text` on the conflict copy,
  `s3-scratch.log:225-226`
  `SCRATCH_CENSUS {... "editable_text": 1, ... "rendered_text": 2 ...}` and
  `SCRATCH_BADGES []`. The focused copy enters `editing` and drops its badge —
  no regression.

## Claim 4 — the badge's `block_id` reaches the bounds registry: CONFIRMED

- Green baseline `s1.log:6-7` (both assertions are on `(entity, words)` sets:
  `pairing_conflict_badge_windowed.rs:232-240,264-272`).
- Break probe: removed `#{block_id: col("id")}` from the yaml's `badge(...)`
  call (legal — the builder's arg is `Option<String>`), restored by sha256
  (`c6392fcf…` before and after). Both tests went red naming exactly the missing
  entity: `s2-break-blockid.log:33-34`
  `left: {("", "kept from this device before pairing")}` /
  `right: {("block:pair-conflict-owner-before-pairing", "kept from this device before pairing")}`
  and `:21-22` the same for `block:pcv-owner-before-pairing`;
  `:46 test result: FAILED. 0 passed; 2 failed`. The label alone still paints, so
  only the entity binding is under test — which is what the claim says.

## Claim 5 — gates: CONFIRMED, failure set EMPTY

- `cargo fmt --all --check`: `s1.log:3 FMT_RC0` (exit 0).
- `just keystone-smoke`: `s5-gates.log:199`
  `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.00s`,
  engagement line `:196` `inv-no-declared-column-absent=28/28` (fully selected,
  all passed; the lane quoted 4/4 at a different case count).
- `cargo nextest run --no-fail-fast -p holon-frontend -p holon-loro -p holon-app`:
  `s5-gates.log:1424` `Summary [86.742s] 1116 tests run: 1116 passed (1 slow), 4 skipped`.
  Zero failures — trivially a subset of the allowed set. No `connection lost`
  iroh red occurred, so no 3x isolation replay was needed.

## Claim 6 — remaining gaps stated honestly: CONFIRMED, but incomplete

The report's three gaps are accurate:
- Rule-head copy not org-seedable — verified: a rule head is a `#+BEGIN_SRC`
  block and the yaml's `rule_card` sits at priority 0, so 3 > 0 holds by
  construction but is not measured. Honest.
- "The query is text, not a click" — matches the code: `DegradedToast` carries
  no action field.
- `dogfood-explorer` not run — still owed.
Gaps the report does NOT state are listed below.

---

# DEFECTS

None. Every rev-2 claim I probed held, and rev 1's DEFECT D1 (the query
truncated out of the toast) is fixed and pinned by both the lane's test and my
independent 82/192-char scratch variant.

# GAPS

**G1' — the `Page`-collision windowed test runs against a DEGRADED ingest.**
`PAGE_VARIANT_ORG` (`frontends/gpui/tests/pairing_conflict_badge_windowed.rs:88-99`)
seeds a `:Page:`-tagged child under a non-page parent, which production ingest
REFUSES on every run: `s1.log` (and `s2-break-blockid.log:11-18`)
`REFUSING write-back ... UNRESOLVABLE INGEST DROP: 1 of 2 on-disk block(s) ...
(name_chain failed loud — a prohibited page-under-non-page topology).
Unresolvable: ["block:pcv-owner-before-pairing"]`, plus
`OrgMode initial scan failed for 1 file(s)` and a quarantine. The badge
assertion still passes, but the test's environment is a file the vault has
quarantined, and the test harness classes these as
`[test_tracing] UNATTRIBUTED PROBLEM ... [ERROR]`. A real conflict copy of a
`Page` block would sit under a page parent; this one cannot.

**G2' — no tier reads the property off a block the pairing op actually wrote.**
The headless test judges only the JSON payload (count + query string); the
windowed tests seed the property from org and never run the pairing op. The
seam "the op writes `pairing_conflict_of` onto the copy the count names" is
still covered only by the plan-level unit tests at
`crates/holon-loro/src/device_pairing_op.rs:1181-1201`.

**G3' — no tier connects the disclosed count to the badge.** Rev-1 G1 is
narrowed, not closed: headless now proves `conflict_copies == 1`, windowed
proves a seeded copy wears a badge, but nothing runs a pair and then paints the
resulting blocks.

**G4' — report claim imprecision (not a defect).** "The `layout_insta` badge
snapshot did NOT move again" is true only relative to rev 1: the working-copy
diff of
`frontends/gpui/tests/snapshots/layout_insta____snapshot_row_with_icon_text_badge.snap`
does add a nested `badge 36x22` line versus `@-`.

**G5' — rev-1 gaps closed.** Rev-1 G5 (badges record no `entity_id`) is fixed
and pinned (claim 4). Rev-1 G4 (priority-0 tie with `rule_card`) is fixed by the
priority-3 move; the `Page` half is measured, the rule-head half is not (the
report's own first gap).

**G6' — `dogfood-explorer` still owed** (report discloses it).
