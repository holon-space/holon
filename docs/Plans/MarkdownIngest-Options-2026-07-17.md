# Markdown Ingest Wiring — Design Options (2026-07-17)

*Decision-options doc for Martin's morning review. Not an implementation. Every
load-bearing claim is cited `file:line`. Read
[docs/Architecture/Model.md](../Architecture/Model.md) and the prior ruling
[docs/Proposals/ForeignVaultCompat-2026-07-12.md](../Proposals/ForeignVaultCompat-2026-07-12.md)
first — this doc updates the "wiring" gap that ruling left open.*

## The dogfood finding

`.md` files are never scanned, so LogSeq/Obsidian vaults are unsupported —
**despite `crates/holon-markdown` existing and parsing them**. The adapters were
built (the ForeignVaultCompat spike, Inc 2/3) but **never wired into the running
app**. This doc is about closing *that* gap, and choosing how far the round-trip
should go, in light of the O1–O5 rulings already on record.

> Note: `docs/Architecture/Model.md:40` still says `holon-markdown` "was
> implemented then removed 2026-07-06 as unwired dead code." **That line is
> stale** — the crate is back on disk (`crates/holon-markdown/`, 6 source files)
> and is a workspace member (`Cargo.toml`), with Obsidian + LogSeq adapters and
> fixture tests. Model.md should be corrected.

---

## What EXISTS today

### The adapter seam (present, generic)

`FileFormatAdapter` (`crates/holon-core/src/file_format.rs:61-185`) is the
pluggable parse/render seam — 10 methods: `extensions()`, `parse()`,
`render_document()`, `render_blocks()`, `doc_id_from_content()`,
`build_block_params()`, `content_differs()`, `sync_document_metadata()`,
`check_writeback_lossless()`, `writeback_drops()`. `OrgFormatAdapter` implements
it with **full read+write** (`crates/holon-orgmode/src/file_format.rs:37-149`:
`extensions()=["org"]`; `render_document`/`render_blocks` delegate to
`OrgRenderer`; `check_writeback_lossless` does block-preservation grounding).

### The wiring gap (the actual bug)

There is **no `FormatRegistry`**. The DI container holds a *single* optional
`Arc<dyn FileFormatAdapter>` and defaults it to `OrgFormatAdapter::new()`
(`crates/holon-orgmode/src/di.rs:395-401`). `holon-markdown` is **not even a
dependency of `holon-app`** (`crates/holon-app/Cargo.toml` lists `holon-orgmode`,
not `holon-markdown`), and `ObsidianMarkdownAdapter` is referenced **only in
tests** — never in app wiring.

**Two extension filters, both org-only:**
1. Directory scan itself is **format-agnostic**: `walk_directory`
   (`fs_port.rs:113-140`) uses `ignore::WalkBuilder` (gitignore-aware, no
   hardcoded extension). It *does* see `.md` files.
2. They are dropped downstream: `FileSyncController` retains only files whose
   extension matches `self.format.extensions()`
   (`file_sync_controller.rs:2591-2596`, `:2595 exts.contains(&e)`), and the org
   watcher independently hardcodes `.org`
   (`crates/holon-orgmode/src/file_watcher.rs:31,89`). Since the only wired
   adapter advertises `["org"]`, every `.md` file is scanned then silently
   dropped.

So the fix is exactly the ForeignVaultCompat **Inc 0** that was ruled but never
landed: a registry that routes per-file by extension, plus registering the
markdown adapter(s). This is confirmation of the plan's own gap analysis
(`ForeignVaultCompat-2026-07-12.md:93-104`).

### What the markdown adapters parse today (read side works)

`ObsidianMarkdownAdapter` (`crates/holon-markdown/src/obsidian.rs`, +`inline.rs`,
+`logseq.rs`) already parses into the **same `Block` + `MarkSpan` substrate as
org**:

- **Frontmatter** (YAML scalars, inline+block lists, `tags`/`aliases`, typed
  properties) → document properties + tag/alias edge fields
  (`obsidian.rs:44-103`).
- **Headings** `#`–`######` nested into the tree; trailing `^anchor` → block id
  (`obsidian.rs:213-231`).
- **List items** `-`/`*`/`1.` with `[ ]`/`[x]` checkbox → task marker prefix →
  `task_state` (`obsidian.rs:282-292,437-446`).
- **Paragraphs**, **callouts** `> [!note]`, **comments** `%%…%%`, **fenced code**,
  **embeds** `![[…]]` → disclosed **opaque blocks** (never dropped;
  `obsidian.rs:239-333`).
- **Inline marks** (`inline.rs`) onto the marks model — this is the load-bearing
  mapping onto the [links-ruling]:
  - `[[Page]]` / `[[Page|alias]]` → `Link` mark, name target
    (`inline.rs:120-136`); `#tag` / `#[[multi word]]` → hoisted to the **tag edge
    field** (`inline.rs:183-211`).
  - `((uuid))` (LogSeq block-ref) → `Link` mark, internal-id target
    (`inline.rs:138-159`).
  - `[text](url)` → `Link` mark, external url (`inline.rs:161-181`).
  - `**b**`/`*i*`/`~~s~~`/`==h==`/`` `code` `` → `Bold`/`Italic`/`Strike`/
    `Underline`/`Code` `InlineMark`s (`inline.rs:87-97,213-231`). **Note:** `==…==`
    currently maps to `Underline`, but O3 was ruled to add a distinct `Highlight`
    variant (`ForeignVaultCompat…:267,319-322`) — the impl is behind that ruling.

These marks flow to the **same `block_links` junction** as org links
(`block_links.sql`: `source_block_id, target, kind, resolved_id`; `kind` ∈
`page`/`block`/`tag`; soft targets, no FK, lazy page-create). So a `[[wiki-link]]`
ingested from markdown resolves through the identical backlink machinery as an
org `[[link]]` — the substrate is genuinely shared.

### The write-back side (currently read-only)

- `ReadOnlyWriteGuard` (`crates/holon-markdown/src/lib.rs:49-87`) is the
  controller-level tier gate: `.may_write()` returns false for foreign files.
- `render_document`/`render_blocks` **panic** ("read-only (Tier R/O)")
  — `obsidian.rs:342-355`; `check_writeback_lossless` / `writeback_drops`
  **refuse unconditionally** — `obsidian.rs:379-407`. There is **no
  markdown-to-disk render path at all** today.

This is deliberately the ADR-0025-honest floor: no Holon op grounds a write to a
file Holon didn't produce, so any write to a foreign file is loss by definition
(`ForeignVaultCompat…:157-163`).

### The tension the options must resolve

**O4 was already ruled (2026-07-13): read AND write for both dialects, WITHOUT
source-byte spans — via a "convergent canonical form"**
(`ForeignVaultCompat…:267-312`). The serializer targets the *foreign app's own
normal form*, so re-saving is a fixed point in ≤1 pass (no ping-pong); it writes
only on a semantic (AST) change, never a byte diff; unmodeled syntax is carried
as opaque blocks re-emitted verbatim; fidelity is tested against a golden corpus
(mldoc = LogSeq's own parser as a CI oracle, Tier 1 feasible ~0.5-1 day). **The
current R/O code is therefore *behind* the ruling, not the target.** The options
below are really "how much of the ruled O4 target do we build now, given the
dogfood pain is just that nothing is wired."

Relevant standing rulings (`ForeignVaultCompat…:261-270`): **O1** LogSeq-org shim
first, then LogSeq-md, Obsidian incrementally; **O2** one `holon-markdown` crate,
dialects **declarative** (YAML-over-one-engine); **O4** read+write via convergent
canonical form; **O5** single-vault now, multi-vault later. And the O4 canonical
form is the org-side precedent already shipping: **convergent canonical form with
no byte spans** (`:REQUIRES:` normalizes `:BLOCKED-BY:` in one pass —
`ORG_SYNTAX.md:105-134`).

---

## The options

### Option A — Full md ingest + write-back parity (the ruled O4 target)

**What it concretely is:** build the registry seam (Inc 0), register both
markdown adapters, and implement `render_document`/`render_blocks` to the
**convergent canonical form** so a Holon edit writes back into the `.md` file in
the foreign app's own normal form. Holon *owns* the markdown vault the way it owns
an org vault.

*Worked example:* user opens an Obsidian vault, edits a task's text and checks a
checkbox in Holon; on consolidation Holon rewrites that `.md` file in Obsidian's
normal form (`- [x] …`, canonical frontmatter). Obsidian re-reads it with no
re-normalization (fixed point). Files Holon only read are never rewritten
(write-on-AST-change, `:284`). A `{{dataview}}` block round-trips verbatim as an
opaque block.

**Decisive tradeoff:** *the only option that lets a user actually work in their
existing vault, at the cost of a new fidelity-oracle apparatus and real ADR-0025
risk surface.* It delivers what "support LogSeq/Obsidian vaults" really means —
edits land back in the vault. But it needs the mldoc golden-corpus oracle
(`:290-308`), the `Highlight` mark (O3), and per-dialect canonical serializers;
and every serializer bug is a *foreign-file corruption* risk, which is why the
ruling gates it behind byte-stability CI. Obsidian is hardest (no faithful
headless parser — Tier 3, manual golden capture only, `:302-306`), so realistic
sequencing is LogSeq-md write-back first, Obsidian write-back last or never.

**A recommendation for A rests on:** committing to the mldoc CI oracle as the
gate (Tier 1, feasibility already evaluated as ~0.5-1 day, `:290-298`), and
accepting LogSeq-md as the first write-back target with Obsidian staying read-only
longer. It is the ruled direction, but it is materially more than the dogfood bug
requires.

### Option B — Read-only ingest, disclosed (edits stay in Holon / optional org sidecar)

**What it concretely is:** build the registry seam (Inc 0), register the markdown
adapters, wire the **existing R/O path** end-to-end. `.md` files ingest into
blocks; Holon-side edits live in Holon's own store and **never touch the foreign
file** (the `ReadOnlyWriteGuard` already enforces this — `lib.rs:49-87`,
`obsidian.rs:342-355`). Disclosed via the per-vault/per-file tier banner
(`ForeignVaultCompat…:175-190`). Optional variant: Holon-side edits are persisted
to an **org sidecar** (Holon's own `.org` mirror) so edits are durable in a
format Holon *can* write, leaving the foreign `.md` untouched.

*Worked example:* user opens a LogSeq vault; all pages/journals/tasks/links
appear and are navigable, backlinks resolve. A banner reads "LogSeq vault, 342
files, Tier R/O — 12 files contain unsupported constructs." Edits are visible in
Holon; the `.md` files on disk are unchanged. (Sidecar variant: the edit is
written to `<vault>/.holon/…​.org`.)

**Decisive tradeoff:** *the smallest change that fixes the dogfood bug and is
provably safe, at the cost of not being a real two-way vault.* It is almost
entirely wiring — the parse side already works and the guard already refuses
writes — so it is the fastest path to "LogSeq/Obsidian vaults open in Holon,"
with **zero foreign-file corruption risk** (the strongest ADR-0025 posture). The
cost: it is a read mirror, not a working vault; a user editing in Holon and
expecting their Obsidian to update will be surprised (mitigated by the disclosure
banner, and by the sidecar variant for durability). The sidecar variant adds a
second-source-of-truth question (which file wins if both change?).

**A recommendation for B rests on:** accepting "read mirror for v1" as the
product stance for foreign vaults (explicitly floated as O4's fallback,
`:323-324`), and whether the disclosure banner is enough or a durable sidecar is
needed. It is the lowest-risk unblock and a strict prefix of Option A (A = B +
canonical write-back), so B-now-A-later loses no work.

### Option C — md → org one-time import (convert the vault)

**What it concretely is:** a one-shot importer that parses the `.md` vault and
**writes out `.org` files** (Holon's native, fully-round-tripping format), after
which the vault is an ordinary Holon org vault. No per-file markdown adapter in
the steady-state runtime; markdown is an *import format*, not a *live format*.

*Worked example:* user runs "Import Obsidian vault"; Holon reads every `.md`,
converts to `.org` (marks → org link syntax, frontmatter → `:PROPERTIES:`,
`- [x]` → org `DONE`), and the user henceforth works in `.org`. The original `.md`
files are left as-is (import copies, does not consume) or archived.

**Decisive tradeoff:** *sidesteps the entire foreign-write-back problem by making
everything native, at the cost of divorcing the user from their existing tool.*
Because the output is `.org`, all of Holon's mature read+write machinery applies
immediately (`OrgFormatAdapter` full round-trip) — no new serializer, no mldoc
oracle, no R/O tier. But the user **leaves LogSeq/Obsidian behind**: their vault
is now org, their other tools can't read it, and any lossy mapping (e.g. Obsidian
callouts, `==highlight==`, embeds) is baked in at import time rather than
disclosed live. It is a migration tool, not vault compatibility.

**A recommendation for C rests on:** the target user being someone *switching to
Holon* rather than *keeping their existing vault*. If the dogfood intent is "I
want to try Holon on my notes," C is fine; if it is "I want Holon to coexist with
my Obsidian," C is wrong (it is a one-way door).

---

## Comparison

| Dimension | A — full parity (O4 target) | B — read-only ingest | C — md→org import |
|---|---|---|---|
| Fixes "md never scanned" | yes | **yes (smallest)** | yes (via conversion) |
| Effort over the wiring seam | serializers + mldoc oracle + `Highlight` | **~just wiring** (parse+guard exist) | org emitter for md constructs |
| Edits land back in `.md` | **yes** (canonical form) | no (Holon store / org sidecar) | n/a (vault becomes org) |
| Foreign-file corruption risk | real (gated by byte-stability CI) | **none** (guard refuses writes) | none (leaves .md untouched) |
| Keeps user in LogSeq/Obsidian | **yes** | yes (read mirror) | **no** (migrated to org) |
| Matches O4 ruling | **is the ruling** | ruled fallback (`:323`) | orthogonal (not compat) |
| Obsidian feasibility | hardest (Tier 3, manual golden) | same as LogSeq (read only) | same as LogSeq (one-shot) |
| Relationship | = B + write-back | strict prefix of A | independent |

---

## Recommendation (for Martin to rule)

**Ship B now (wire the registry + register the R/O adapters), on the explicit
path to A.** Reasoning:

- The dogfood bug is *purely a wiring gap* — the parse side already works and the
  scan already sees `.md`. B is mostly Inc 0 (the `FormatRegistry` + per-file
  routing seam that was ruled but never landed) plus registering the existing
  adapters, and it is the fastest, **zero-corruption-risk** way to make
  LogSeq/Obsidian vaults open in Holon.
- B is a **strict prefix of A** (A = B + convergent-canonical write-back), so
  choosing B now forecloses nothing and matches the increment plan's own order
  (Inc 0 → LogSeq-org shim → LogSeq-md R/O → Obsidian R/O → tier gate → keystone,
  then Inc 6 write-back). It also aligns with O1 (LogSeq first) and O2 (one crate,
  declarative dialects).
- A is the ruled *destination* (O4: read+write, convergent canonical form) and
  should follow B directly for **LogSeq-md** (mldoc oracle makes it testable),
  with Obsidian write-back deferred (no faithful headless parser).
- C is not recommended as the primary answer — it is a *migration* feature, not
  *vault compatibility*, and it conflicts with the O4 ruling that Holon should
  read+write foreign formats in place. It is worth offering **separately** as an
  explicit "convert to Holon-native" action for users who are switching, but it
  does not satisfy "support LogSeq/Obsidian vaults."

This recommendation **rests on** the product stance that foreign vaults should be
worked in *live* (O4's ruling), with a read-only first step being an acceptable,
disclosed v1 rather than the end state. If Martin's actual intent is "R/O forever
is fine for foreign vaults" then B *is* the destination and A is dropped; if the
intent is "users migrate to Holon-org," C moves up.

---

## Open questions requiring a ruling

1. **R/O v1 vs straight-to-write-back:** ship B (read-only) as the disclosed v1
   and follow with A for LogSeq-md, or hold until A is ready? (O4 ruled read+write
   as the target; O4's own §6b left "R/O-forever acceptable for v1?" open,
   `:323-324`.)
2. **Sidecar for Holon-side edits (B variant):** when a user edits a foreign-vault
   block in Holon, do those edits (a) live only in Holon's store, (b) persist to a
   `.org` sidecar, or (c) block editing entirely until write-back lands? Each has a
   different "which file is truth" answer.
3. **`==highlight==` (O3):** confirm the ruled new `Highlight` `InlineMark`
   variant lands (touches the mark enum + every renderer) vs the current
   `Underline` aliasing in `inline.rs`.
4. **Obsidian write-back appetite:** given no faithful headless Obsidian parser
   exists (Tier 3, `:302-306`), is Obsidian **read-only indefinitely** while only
   LogSeq-md gets write-back — or is a manual golden-corpus enough to attempt it?
5. **Vault-flavor discrimination + multi-vault (O5):** the registry needs a
   one-time vault classifier (`.obsidian/` ⇒ Obsidian, `logseq/config.edn` ⇒
   LogSeq — `:113-116`); and is opening a *second* (foreign) vault alongside the
   org vault in scope now, or one-vault-at-a-time?
6. **C as a separate feature:** should a one-shot "Import → convert to org"
   action exist in parallel with live compat, for users switching rather than
   coexisting?
7. **Model.md correction:** line 40 ("holon-markdown … removed 2026-07-06 as
   unwired dead code") is stale and should be updated to reflect the re-added
   crate.
