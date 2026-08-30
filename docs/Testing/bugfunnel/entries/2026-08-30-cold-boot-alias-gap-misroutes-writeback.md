---
id: 2026-08-30-cold-boot-alias-gap-misroutes-writeback
date: 2026-08-30
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  A doc's write-back was aimed at a DIFFERENT doc's file, and only the
  ADR-0025 removal guard stopped 31 blocks of a real vault page from
  being erased.
---

## Bug

Booting release `holon-gpui` (built from `main` 904c2563) against Martin's
production vault quarantined one file:

```
ERROR org.on_block_feed{messages=4788}:org.on_block_changed{doc_id=block:93c4e460-…}:
[FileSyncController] write-back would remove on-disk blocks that no op sanctioned
— QUARANTINING this file from write-back so its lossy projection is not rendered
over disk.
path=…/holon-pkm/Projects/DBG/Agentic DPL/Prototype.org
error=UNGROUNDED WRITE-BACK REMOVAL: 31 of 31 on-disk block(s) would be DELETED …
```

Found by Martin dogfooding. One event, one file, in a 4788-message feed.
Log: `…/scratchpad/holon-app2.log:473`.

The framing "the projection of that file is empty" is wrong, and that
matters: the projection was not empty, it belonged to **another
document**. The doc in the error, `93c4e460`, is the root of

    Projects/Agentic DPL/Prototype.org      — 43 bytes, an `#+ID:` line and nothing else

while the path in the error is

    Projects/DBG/Agentic DPL/Prototype.org  — 3373 bytes, 31 blocks, root `d7999def`

Rendering the 0-block doc was correct. Aiming it at the 31-block file was
not. The guard held, and no vault bytes were lost.

## Root cause

Three facts compose. Verified against a copy of the production database
(`…/scratchpad/dbrepro/`, opened with `PRAGMA writable_schema=ON` so stock
`sqlite3` skips the matview definitions it cannot parse).

**1. Two vault files carry the same `#+ID:`, so ingest collapsed two
directories into one doc node.** `Projects/Agentic DPL.org` and
`Projects/DBG/Agentic DPL.org` are separate inodes with byte-identical
content, both headed `#+ID: 9464fbf0-…`. Ingest resolves identity from
`#+ID:` (`file_sync_controller.rs:2463`) and accepts the second file as
the same document, silently. The vault holds **8** such duplicate-`#+ID:`
pairs today; this is the only one that has fired.

**2. The two distinct `Prototype` docs therefore share one parent, and so
one name chain.** From `block_raw`:

| doc | parent | content |
|---|---|---|
| `93c4e460` | `9464fbf0` | Prototype |
| `d7999def` | `9464fbf0` | Prototype |
| `9464fbf0` | `e01eb08a` | Agentic DPL |
| `e01eb08a` | `aef282e2` | DBG |
| `aef282e2` | — | Projects |

The collapsed parent kept whichever home ingest recorded last, `DBG`, so
`name_chain(93c4e460)` = `[Projects, DBG, Agentic DPL, Prototype]` and
`page_file_from_name_chain` (`vault_path.rs:30`) yields exactly the path
in the error. Two docs, one derived path — the derivation is not
injective, and nothing asserts that it is. `block_raw` holds a second
such pair already (`Holon` twice under `Projects`).

**3. The cold-boot fast path leaves the alias registry empty, so the
non-injective fallback is what actually runs.** `doc_id_to_path`
(`file_sync_controller.rs:6099`) asks the alias registrar first and only
falls back to the name chain. The registrar is the injective route: it
maps a doc to the path it was ingested from. But on a boot of an
unchanged vault, `initialize` replays the persisted per-file hashes
(`:1149`) and every unchanged file takes the byte-identity skip at
`:2404`, which returns at `:2434` — **before** the `register_alias` call
at `:2574`. The skip does call `note_doc_home` (`:2431`), and its comment
says it is patching exactly this bypass, but `doc_homes` is not consulted
by `doc_id_to_path` at all. Only half the record was restored.

The alias map is in-memory (`loro_document_store.rs:184`), so nothing
carries it across the boot. Result: after a cold boot, **every** unchanged
doc resolves its write-back path by name chain, and any doc sharing a
chain with another can be aimed at that other doc's file.

`file.document_id` is NULL for all 149 rows, so the SQL side offers no
second path either.

## Missing piece

The cold-boot fast path is exercised once in the suite —
`crates/holon-orgmode/tests/page_rename_retires_old_file.rs`,
`a_page_renamed_after_a_cold_boot_fast_path_still_retires_its_old_file` —
and that test builds its controller with **no alias registrar**. So the
one wiring where the seam breaks (fast path *plus* a registrar present,
which is the shipped Loro wiring) never runs in any test. The test that
covers the skip asserts the `doc_homes` half and cannot see that the
alias half is missing.

Secondary, COVERAGE: no generator emits two vault files carrying the same
`#+ID:`, nor two docs sharing a name chain, so the collapse in fact 1 and
the ambiguity in fact 2 are both ungeneratable.

The keystone PBT cannot reproduce this: it ingests a fresh vault in one
process, where no persisted hash exists to skip on and no duplicate
`#+ID:` is ever authored. Prod/test parity work needed: a second-boot rung
that re-opens a vault over the hashes the first boot stamped, driven
through the Loro wiring rather than the SqlOnly default.

## Remedy

Fixed, not yet landed — the change sits uncommitted in the
`quarantine-triage` workspace. Status stays OPEN until it lands.

Rungs in the existing harness, so no doubles are duplicated —
`crates/holon-orgmode/tests/page_rename_retires_old_file.rs`. They ran
red against the PRISTINE base extracted with `jj file show -r @-`
(`…/scratchpad/red_rungs_on_base.log`) and green after;
the file's other 8 tests stayed green throughout. The harm rung is
driven through BOTH wirings, because the refusal reads `doc_home`, which
exists in every storage mode — the shipped SqlOnly default needs the
protection as much as the Loro one.

`a_cold_boot_fast_path_still_registers_the_docs_alias` — the record the
routing depends on. Runs the neighbouring cold-boot recipe, then asks
for the doc's alias:

```
left: None
right: Some(".../structural-page/pagea.org")
```

`a_namesake_docs_writeback_leaves_the_owners_file_writable` — the harm.
A second live page answers the owner's name chain, and its write-back
reproduces the production failure verbatim against the owner's file:

```
UNGROUNDED WRITE-BACK REMOVAL: 1 of 1 on-disk block(s) would be DELETED …
```

Quarantining that file is the real cost: the owner's own later edit
then never reaches disk, which is what the test asserts.

Fixed, in `crates/holon-filesystem/src/file_sync_controller.rs`:

- **The mis-route.** The cold-boot fast-path skip now registers the
  alias beside its existing `note_doc_home`. That restores the tier
  `doc_id_to_path` asks first, so an unchanged doc is routed to the file
  it was ingested FROM instead of falling through to a name chain, which
  only says where a page of that title WOULD go. It also means a rename,
  which retires the union of the two records (`prior_page_homes`), can
  reach both homes rather than one.
- **The ambiguity.** `refuse_contested_path` rejects a name-chain
  derivation that lands on a file a DIFFERENT doc already homes, naming
  both doc ids and the contested path. It returns `Err`, which
  `on_block_changed` already routes to `disclose_derivation_failure` —
  one loud ERROR per doc, DEBUG on repeat, and this document's write
  skipped while every other document keeps syncing. It applies only to
  `PathIntent::WriteOwnFile`: a residence lookup passes a BLOCK id whose
  chain resolves to its own document's file, and that document homing the
  file is ownership, not a contest.

The two are independent: `note_doc_home` predates this change, so the
owner already had the record the refusal scans, and either fix alone
stops the observed write.

A first cut also made `doc_id_to_path` prefer the `doc_home` record over
the name chain, and applied the refusal for every caller. Adversarial
verification caught three regressions from that shape, all in
`incremental_org_writeback_smoke.rs`. Refusing on residence lookups
re-diagnosed both ungrounded-drop vetoes as unresolvable name chains —
fixed by the `PathIntent` split above. The home tier overwrote a
childless runtime page's identity file with empty bytes, and that one is
why the tier is gone rather than reordered: where the page registry lags,
`materialize_page_identity_file` writes an identity file at the
authoritative chain while `doc_manager.name_chain` still resolves
elsewhere, and preferring the home record aims the document's own
write-back at that identity file — whose render, for a childless page, is
empty. Dropping the tier costs nothing the bug needed, since registering
the alias is what actually restores the routing.

That exposes a latent defect worth its own entry: nothing stops
`on_block_changed` writing an empty render over a non-empty file. Today
it is unreachable only because the two paths differ.

The gate that missed all three ran one test FILE. `--all-targets` on
`holon-orgmode` is what surfaces them and belongs in this lane's gate
list permanently. One caveat on reading that gate green:
`block_params::tests::the_refusal_is_disclosed_exactly_once_per_key` is
order-dependent — it passes in isolation, passed in this lane's combined
runs, and has been observed red in another combined run. It is unrelated
to this change and is not this lane's to fix, so "`--all-targets` clean"
means clean EXCEPT that known single.

Three residuals this fix does not close:

- The `PathIntent` split is advisory, not structural: both intents
  return the same `VaultPath`, so nothing stops a future caller passing
  `LookupResidence` and then writing. Making that unrepresentable wants a
  distinct write-capability return type (parse, don't validate) rather
  than a flag — worth doing when the next caller is added.
- There is an initial-scan window: a contested file can be on disk while
  no home record exists yet, and a write in that window proceeds because
  the scan that would record the home has not landed. The refusal is only
  as good as the records it reads.
- In SqlOnly the alias half of the fix is a no-op — there is no registrar
  to register into. Protection there rests entirely on the refusal plus
  the pre-existing `note_doc_home`, which is why the harm rung is run
  through that wiring too.

Residue, parked for Martin's ruling — **not** implemented: ingest still
accepts the same `#+ID:` from a second file and collapses two vault
directories into one doc node (`:2463`). That is the upstream cause of
this bug and no later layer can undo it, but the vault holds 8 such
pairs today, several of them live-vs-`_archive` copies, so deciding what
happens to them is a data-cleanup call rather than a code call. Until it
is made, the two fixes above contain the damage: the mis-route cannot
happen, and a genuine ambiguity is refused loudly instead of
quarantining a file.
