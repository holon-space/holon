---
id: 2026-08-02-sidebar-shows-two-collapsible-page-rows
date: 2026-08-02
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  The sidebar shows two collapsible page rows with EMPTY titles, whose
  children are the real pages `citrix-STX.BROWSER_AGENT` and `Optimize RAG`.
  They are `block:2285fefd-72b0-2fe8-c125-f7dd82fbefe8` and
  `block:298e1a17-3057-d230-c76b-a40e94f905a0`: `parent_id =
  sentinel:no_parent`, content `''`, tagged `Page`. ROOT CAUSE: both are
  placeholder roots minted by `resolve_parent_or_placeholder`
  (`crates/holon-loro/src/block_cell_registry.rs:622-640`, `standing up an
  EMPTY placeholder root for a parent not yet in the Loro tree`,
  `/private/tmp/holon-cold.log:1278,1548`) while ingesting
  `Projects/Aiuno/Optimize RAG.org` and
  `Agents/citrix/citrix-STX.BROWSER_AGENT.org`. The placeholder is designed to
  be COMPLETED when the real create for that parent arrives — but the parent
  here is the folder page for the containing DIRECTORY, and neither
  `Projects/Aiuno.org` nor `Agents/citrix.org` exists on disk (unlike
  `Projects/Holon.org`, `Projects/DBG.org`, `Areas.org`, …). No create ever
  arrives, so the husk is permanent. DOWNSTREAM DAMAGE, same root cause:
  `doc_id_to_path`
  (`crates/holon-filesystem/src/file_sync_controller.rs:4904-4921`) falls back
  to `self.root_dir.join(chain.join("/")).with_extension("org")` when the
  alias lookup misses, and the husk contributes an EMPTY element to the name
  chain — so (a) for the husk itself the chain is `[""]`, `root_dir.join("")`
  is `root_dir`, and `.with_extension("org")` produced
  `/Users/martin/Workspaces/pkm/holon-pkm.org` — a real 43-byte file Holon
  wrote OUTSIDE the vault (`log:2069,2073`, `Wrote block changes to
  …/holon-pkm.org`); and (b) for the child docs the chain is `["", "Optimize
  RAG"]`, whose `join("/")` starts with `/`, and `PathBuf::join` with an
  ABSOLUTE component DISCARDS the base — yielding `/Optimize RAG.org` and
  (after `with_extension` also ate the `.BROWSER_AGENT` suffix)
  `/citrix-STX.org` at the filesystem root, both EROFS. Write-back for those
  two REAL vault pages is now DISABLED for the session by the EROFS skip
  (`log:2070,2074`), so edits to them never reach disk.
source_line: 1135
---

## Bug

(dogfood, live GPUI on the real vault) The sidebar shows two collapsible
page rows with EMPTY titles, whose children are the real pages
`citrix-STX.BROWSER_AGENT` and `Optimize RAG`. They are
`block:2285fefd-72b0-2fe8-c125-f7dd82fbefe8` and
`block:298e1a17-3057-d230-c76b-a40e94f905a0`: `parent_id =
sentinel:no_parent`, content `''`, tagged `Page`. ROOT CAUSE: both are
placeholder roots minted by `resolve_parent_or_placeholder`
(`crates/holon-loro/src/block_cell_registry.rs:622-640`, `standing up an
EMPTY placeholder root for a parent not yet in the Loro tree`,
`/private/tmp/holon-cold.log:1278,1548`) while ingesting
`Projects/Aiuno/Optimize RAG.org` and
`Agents/citrix/citrix-STX.BROWSER_AGENT.org`. The placeholder is designed to
be COMPLETED when the real create for that parent arrives — but the parent
here is the folder page for the containing DIRECTORY, and neither
`Projects/Aiuno.org` nor `Agents/citrix.org` exists on disk (unlike
`Projects/Holon.org`, `Projects/DBG.org`, `Areas.org`, …). No create ever
arrives, so the husk is permanent. DOWNSTREAM DAMAGE, same root cause:
`doc_id_to_path`
(`crates/holon-filesystem/src/file_sync_controller.rs:4904-4921`) falls back
to `self.root_dir.join(chain.join("/")).with_extension("org")` when the
alias lookup misses, and the husk contributes an EMPTY element to the name
chain — so (a) for the husk itself the chain is `[""]`, `root_dir.join("")`
is `root_dir`, and `.with_extension("org")` produced
`/Users/martin/Workspaces/pkm/holon-pkm.org` — a real 43-byte file Holon
wrote OUTSIDE the vault (`log:2069,2073`, `Wrote block changes to
…/holon-pkm.org`); and (b) for the child docs the chain is `["", "Optimize
RAG"]`, whose `join("/")` starts with `/`, and `PathBuf::join` with an
ABSOLUTE component DISCARDS the base — yielding `/Optimize RAG.org` and
(after `with_extension` also ate the `.BROWSER_AGENT` suffix)
`/citrix-STX.org` at the filesystem root, both EROFS. Write-back for those
two REAL vault pages is now DISABLED for the session by the EROFS skip
(`log:2070,2074`), so edits to them never reach disk.

## Missing piece

The keystone cannot generate the triggering vault SHAPE: every org filename
the generator draws matches `[a-z_]+_[0-9]+\.org`
(`crates/holon-integration-tests/src/pbt/generators.rs:359,545,586,672,748`)
— FLAT, no subdirectory component anywhere in the catalog, so 'a directory
containing org files but no sibling `<dir>.org` companion' is unreachable,
and with it the whole placeholder-never-completed state. Secondary ORACLE:
no invariant asserts (i) every `Page` has a non-empty title, or (ii) every
write-back path stays INSIDE `root_dir` — the second is what let a file be
written outside the vault without a single test noticing. Rungs that would
close it: (a) a `WriteOrgFile` filename arm with a nested `dir/name.org`
shape plus a variant that omits the companion `dir.org`; (b) an invariant
`no Page-tagged block has empty content`; (c) a cheap, always-on assertion
in `doc_id_to_path` that the produced path is a descendant of `root_dir`.

## Remedy

OPEN — diagnosis only (2026-08-02 triage lane). Fix direction, two
independent pieces: (1) make the folder-page husk impossible — either mint
the directory page with its DIRECTORY NAME as content (it is known at ingest
time from the file path) instead of an empty placeholder, or refuse to
`Page`-tag a placeholder until it is completed; (2) make the path derivation
fail loud — `name_chain` must reject an empty element rather than return it,
and the `root_dir.join(...)` result must be asserted to live under
`root_dir` (parse-don't-validate: a `VaultPath` newtype that cannot
represent an escape). Piece (2) is worth landing on its own regardless of
(1): it is the difference between a cosmetic empty row and Holon writing
files outside the vault.
