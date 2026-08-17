---
id: 2026-07-28-cold-boot-quarantines-folder-companion-only
date: 2026-07-28
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Cold boot QUARANTINES folder-companion `#+ID:`-only org files (`X.org`
  beside a directory `X/`) with `holon-identity-collision ... (held title "",
  requested "Music")`. The "different entity" holding the id is a Loro
  `create_placeholder_root` — a content-less, tag-less node stood up when a
  child's `create_in_tree` reached its parent before the async SQL->Loro apply
  landed — whose projection wiped the page's content and `Page` tag, making it
  invisible to `LiveDocumentManager`'s `tag='Page'` view. Lifting the refusal
  exposed a MASKED and worse defect: the page stayed at the Loro root,
  collapsing its name chain, so write-back RELOCATED the user's files
  (`Areas/Music.org` -> `Music.org`). Fixed by
  `Recognition::UnnamedPlaceholder` (blank holder = adopt, not collide; real
  renamed-holder collisions still fail loud) plus placeholder content-and-home
  completion in `block_cell_registry`.
source_line: 1115
---

## Bug

Cold boot QUARANTINES folder-companion `#+ID:`-only org files (`X.org`
beside a directory `X/`) with `holon-identity-collision ... (held title "",
requested "Music")`. The "different entity" holding the id is a Loro
`create_placeholder_root` — a content-less, tag-less node stood up when a
child's `create_in_tree` reached its parent before the async SQL->Loro apply
landed — whose projection wiped the page's content and `Page` tag, making it
invisible to `LiveDocumentManager`'s `tag='Page'` view. Lifting the refusal
exposed a MASKED and worse defect: the page stayed at the Loro root,
collapsing its name chain, so write-back RELOCATED the user's files
(`Areas/Music.org` -> `Music.org`). Fixed by
`Recognition::UnnamedPlaceholder` (blank holder = adopt, not collide; real
renamed-holder collisions still fail loud) plus placeholder content-and-home
completion in `block_cell_registry`.

## Root cause

cold boot against Martin's real vault refused THREE org files with
`holon-identity-collision: id block:<id> is already held by a different
entity (held title "", requested "Music")` and QUARANTINED them from
write-back (6 ERRORs; `Areas.org`, `Areas/Music.org`, `Areas/Music/Audio
Processing.org`). Shape: a folder-companion `#+ID:`-only org file — `X.org`
sitting next to a directory `X/`. Such a file is HEALTHY output, not
corruption: `render_document_header` emits `#+TITLE:` only from an explicit
title property, so a child-less page whose title equals its file stem
renders as a bare `#+ID: <uuid>`. Root cause is a three-layer chain, each
layer individually reasonable: (1) the vault walk reaches `Areas/Music/Audio
Processing.org` FIRST, whose `resolve_dir_page_chain` create-if-absents the
`Areas` and `Music` pages in SQL (correct content) via `create_forcing_id`;
(2) the immediately following `create_in_tree` for that file's doc block
finds its parent `Music` NOT YET in the Loro tree — the SQL->Loro apply is
async — so `block_cell_registry.rs` stands up a `create_placeholder_root`, a
node carrying NO content and NO tags, whose outbound projection writes
content "" and drops the `Page` tag on the page's real `block_raw` row; (3)
`Areas/Music.org`'s own ingest then can't see that row at all, because
`LiveDocumentManager`'s page view is `block_raw JOIN block_tags WHERE
tag='Page'` — so `get_by_id` misses, `create_forcing_id` issues a create,
and the ADR 0029 D1b minter reads holder title "" != "Music" and refuses
FAIL-LOUD. The refusal is the interim policy working exactly as specified —
on a premise that was false: an empty holder is not a rival entity, it is an
unnamed structural placeholder for THAT VERY ID, so there is no name to
clobber. COVERAGE primary and it is unambiguous: the keystone's filename
generator is the regex `[a-z_]+_[0-9]+\.org` (`generators.rs:351`) — FLAT,
no directory component anywhere in the alphabet — so no transition sequence
can place a companion file beside a same-named folder, and every generated
file carries >=1 block so the `#+ID:`-only shape is unreachable too. NOT an
oracle gap: had the shape been generated, `inv-no-observed-errors` would
have caught the quarantine ERROR immediately. Deliberately NOT classified
ENVIRONMENT — the failing code path (Loro placeholder roots, async SQL->Loro
window) is fully live in the keystone wiring; only the vault SHAPE is
missing. SECOND, MORE SEVERE defect found while fixing, and it was MASKED by
the first: once the collision is lifted, the ingest completes but the page
stays parented at the Loro tree ROOT, because the placeholder was never
re-homed and its own parent (`Areas`) was equally unresolvable at that
moment. Its name chain collapses to `Music`, and write-back RELOCATES the
user's files — `Areas/Music.org` -> `Music.org`, `Areas/Music/Audio
Processing.org` -> `Music/Audio Processing.org`. Verified by direct vault
listing in the regression test; the quarantine had been the only thing
preventing on-disk relocation. Fixed in three parts: (a)
`Recognition::UnnamedPlaceholder` — a holder whose title is blank is
recognized as an unnamed placeholder, never a collision, and blessed so the
create ADOPTS it (disclosed by a WARN at the minter, and the create's
`INSERT ... ON CONFLICT(id) DO UPDATE` completes the row); a real
renamed-holder collision stays fail-loud, unchanged; (b)
`LoroBackend::complete_placeholder_content` + a content half to the existing
edge-field reconcile in `create_entity`, so a content-less node is completed
the moment its real create arrives (clobber-free by construction — an empty
node has nothing to lose); (c) the completion re-homes the node through the
SAME resolve-or-stand-up-a-placeholder path the create uses (extracted as
`resolve_parent_or_placeholder`), so a whole ancestor chain reached
bottom-up lands homed instead of stranding pages at the root. Regression:
`crates/holon-integration-tests/tests/idonly_folder_companion_identity_collision.rs`,
red with the verbatim dogfood error text before the fix, green after, and it
asserts titles, on-disk paths, AND a touch-rewrite replay phase that would
catch a Loro node left empty. SIBLING of the still-open 2026-07-21 row
"Duplicate sidebar pages" — same missing rung (`Dir.org` coexisting with
`Dir/Child.org`), which is now twice-confirmed as a real-vault shape the
keystone cannot reach; adding a folder-tree ingest arm to the filename
generator would close both.)

## Missing piece

keystone filename generator is the flat regex `[a-z_]+_[0-9]+\.org` — no
directory component, and every generated file carries >=1 block, so neither
a nested vault nor an `#+ID:`-only file is reachable

## Remedy

fixed (regression: `idonly_folder_companion_identity_collision.rs`);
generator rung still missing — shared with the open 2026-07-21
duplicate-folder-page row
