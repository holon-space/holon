---
id: 2026-08-02-mangles-page-titles-containing-dot-replaces
date: 2026-08-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `.with_extension("org")` mangles page titles containing a dot.
  `Path::with_extension` REPLACES the segment after the last `.` rather than
  appending, so a page whose authoritative name-chain leaf is
  `citrix-STX.BROWSER_AGENT` derives the file `citrix-STX.org`, not
  `citrix-STX.BROWSER_AGENT.org`. Three call sites, all in
  `crates/holon-filesystem/src/file_sync_controller.rs`, all on the same
  `root_dir.join(chain.join("/")).with_extension("org")` shape: `:1162` (the
  `on_file_deleted` stale-rename guard, which compares the derived current
  path against the deleted file — a mangled derivation can make a live page's
  path compare EQUAL to an unrelated deleted file, or unequal to its own),
  `:4336` (`materialize_page_identity_file`, the WRITE-BACK seat that decides
  where the page's org file is written and which old file is removed as
  double-homed), and `:4920` (the authoritative-path lookup the write-back
  callers key on). Two distinct harms: (i) round-trip identity break — the
  title that comes out of ingest→write-back is not the title that went in,
  because the file it lands in re-ingests as `citrix-STX`; (ii) COLLISION — a
  page titled `citrix-STX` and a page titled `citrix-STX.BROWSER_AGENT` derive
  the SAME path, so one silently overwrites the other and
  `inv-every-page-has-its-own-file` is violated in production.
  `Agents/citrix/citrix-STX.BROWSER_AGENT.org` is a REAL file in Martin's live
  vault (it is the same file the 2026-08-01 drawer-order row characterizes),
  so this reproduces today. Noticed by agent exploration while fixing an
  unrelated defect in the same function; behavior is currently preserved
  byte-for-byte — nothing has been changed — so this is a LATENT, still-live
  defect. Correct derivation is a plain `format!("{title}.org")` /
  `push`-style append, i.e. `PathBuf::from(format!("{}.org",
  chain.join("/")))` semantics, never `with_extension`.
source_line: 785
---

## Bug

`.with_extension("org")` mangles page titles containing a dot.
`Path::with_extension` REPLACES the segment after the last `.` rather than
appending, so a page whose authoritative name-chain leaf is
`citrix-STX.BROWSER_AGENT` derives the file `citrix-STX.org`, not
`citrix-STX.BROWSER_AGENT.org`. Three call sites, all in
`crates/holon-filesystem/src/file_sync_controller.rs`, all on the same
`root_dir.join(chain.join("/")).with_extension("org")` shape: `:1162` (the
`on_file_deleted` stale-rename guard, which compares the derived current
path against the deleted file — a mangled derivation can make a live page's
path compare EQUAL to an unrelated deleted file, or unequal to its own),
`:4336` (`materialize_page_identity_file`, the WRITE-BACK seat that decides
where the page's org file is written and which old file is removed as
double-homed), and `:4920` (the authoritative-path lookup the write-back
callers key on). Two distinct harms: (i) round-trip identity break — the
title that comes out of ingest→write-back is not the title that went in,
because the file it lands in re-ingests as `citrix-STX`; (ii) COLLISION — a
page titled `citrix-STX` and a page titled `citrix-STX.BROWSER_AGENT` derive
the SAME path, so one silently overwrites the other and
`inv-every-page-has-its-own-file` is violated in production.
`Agents/citrix/citrix-STX.BROWSER_AGENT.org` is a REAL file in Martin's live
vault (it is the same file the 2026-08-01 drawer-order row characterizes),
so this reproduces today. Noticed by agent exploration while fixing an
unrelated defect in the same function; behavior is currently preserved
byte-for-byte — nothing has been changed — so this is a LATENT, still-live
defect. Correct derivation is a plain `format!("{title}.org")` /
`push`-style append, i.e. `PathBuf::from(format!("{}.org",
chain.join("/")))` semantics, never `with_extension`.

## Missing piece

The title ALPHABET, not the transitions. Page-title generation is a
4-element literal pool: `TITLE_POOL = ["Renamed", "Retitled", "Moved",
"Renamed2"]`
(`crates/holon-integration-tests/src/pbt/transitions/rename_page.rs:48`),
consumed by `RenamePage`'s `free_titles` and by `CreatePageAtFreedPath`.
Every entry is dotless and alphanumeric, so `with_extension("org")` is
indistinguishable from an append for every title the state machine can ever
draw, and the existing filename/identity invariants
(`inv-every-page-has-its-own-file`, the title round-trip checks) pass
vacuously. The block-content generators DO emit dots (image `stem.ext`), but
a dot has never reached a page TITLE.

## Remedy

FIXED 2026-08-04 — the three call sites had since been consolidated behind
ONE seat, `VaultPath::page_file_from_name_chain`
(`crates/holon-filesystem/src/vault_path.rs`), so the fix is a single edit:
the LEAF chain segment is pushed as `format!("{segment}.org")` and
`with_extension` is gone. Dotless titles are byte-identical
(`nested_chain_derives_a_descendant_page_file` unchanged). The inverse seat
`path_to_name_chain` (`file_sync_controller.rs`) uses `with_extension("")`,
which strips WHATEVER the last extension is — for the `.org` files this
derivation produces that is exactly equivalent to stripping `.org`, so
title→path→title round-trips for dotted titles
(`citrix-STX.BROWSER_AGENT.org` → `citrix-STX.BROWSER_AGENT`,
`Trailing..org` → `Trailing.`). A SECOND truncating derivation was found and
fixed: `crates/holon/tests/convert_block_to_page_e2e.rs::page_file_path`
RE-IMPLEMENTED `with_extension("org")` inside the title-round-trip oracle,
so that oracle agreed with the bug instead of catching it; it now calls
`VaultPath::page_file_from_name_chain` directly. RED-FIRST EVIDENCE, three
tiers, all red for exactly the truncation before the fix: (a) seat unit
tests `a_dotted_leaf_title_keeps_its_dots` (`left:
"/vault/Agents/citrix-STX.org"` vs `right:
".../citrix-STX.BROWSER_AGENT.org"`) and
`a_dotted_title_does_not_collide_with_its_truncation` (both derive
`/vault/citrix-STX.org`) — `logs/lane-dotted/red-unit-seat.log`; (b)
write-back tier `crates/holon-orgmode/tests/dotted_page_title_writeback.rs`
(2 red → 2 green) — `logs/lane-dotted/red-integration.log`; (c) KEYSTONE,
deterministic hand-authored case `dotted-page-title-owns-its-own-file` (two
sibling pages `citrix-STX` / `citrix-STX.BROWSER_AGENT` via `BlockToPage`)
red on `inv-every-page-has-its-own-file` — "page `b0f4a059-…` owns NO file
(fileless)" — `logs/lane-dotted/red-keystone-handauthored.log`. (d) title
round-trip at the convert boundary, `convert_title_round_trips_dotted` —
`left: "citrix-STX"` vs `right: "citrix-STX.BROWSER_AGENT"`, the exact
divergence this row predicted, proven by reverting the seat
(`logs/lane-dotted/red-convert-e2e-dotted.log`). GREEN after:
`holon-filesystem` 67/67, orgmode 2/2 + `vault_path_escape` 8/8,
`convert_block_to_page_e2e` 9/9, full `just hand-authored` 9/9
(`logs/lane-dotted/green*.log`). MIGRATION verified, NOT improvised:
`a_page_homed_at_the_truncated_file_is_relocated_on_the_next_write` proves
the EXISTING rename/double-homing cleanup in
`materialize_page_identity_file` re-homes a page whose alias still points at
the truncated file — new file written, truncated file removed, alias
re-pointed. GENERATOR COVERAGE DELIBERATELY DEFERRED: dotted entries were
added to `TITLE_POOL` and then REMOVED again. A dotted pool entry cannot be
shown to be drawn — `RenamePage` is eligible only after a `BlockToPage`
mints a page under a page (it fired twice in an 8-case smoke), and NO green
keystone run prints the title it drew, so neither an 8-case smoke nor a
24-case sweep can distinguish "drawn" from "not drawn". Claiming that
coverage would have been aspirational, so the dotted guard is DETERMINISTIC
instead: the hand-authored case `page-renamed-to-a-dotted-title-rehomes`
covers the RENAME/re-home half and `dotted-page-title-owns-its-own-file` the
COLLISION half, both printed and asserted every `just hand-authored` run
(`Applying transition 5/5: RenamePage(… new_title:
"citrix-STX.BROWSER_AGENT")` → `PASSED`,
`logs/lane-dotted/green2-handauthored.log`). Making the random alphabet
dotted remains open and needs harness surgery first: the keystone must log
the transition it applied, otherwise generator coverage is unfalsifiable.
ORIGINAL RED-FIRST PLAN: (1) add dotted entries to `TITLE_POOL` — at minimum
`citrix-STX.BROWSER_AGENT` (the real-vault shape), a bare-dot `a.b`, a
trailing-dot `Trailing.`, and a multi-dot `x.y.z`; keep the existing dotless
entries so the collision case (`A` vs `A.b`) becomes reachable within one
run once `A.b` derives `A.org`. (2) The catching invariant is the page↔file
round-trip identity one: after a `RenamePage`/`CreatePageAtFreedPath`
settle, the page's title re-read from disk must equal the title written, and
the derived path must be injective over the page set — expect RED with
`left: "citrix-STX", right: "citrix-STX.BROWSER_AGENT"` (identity) and a
double-homing red on the collision case, i.e. red for exactly the
`with_extension` truncation and not for an unrelated settle problem. (3) Fix
by replacing all three `with_extension("org")` sites with an append that
never inspects existing dots, and keep the dotted titles in the pool as a
permanent regression guard.
