# Foreign-Vault Compatibility — Obsidian & LogSeq (2026-07-12)

**Status:** PROPOSAL (needs Martin ruling on the tier ladder and increment order)
**Author:** Fable compat stream
**Related:** ADR 0025 (op-grounded projections; external-file-edit boundary), links-ruling
2026-07-10 (marks-as-truth), `FileFormatAdapter` seam (spec 0006 Phase 1),
`docs/Reference/ORG_SYNTAX.md`, `docs/Testing/LogSeqParity-2026-07-10.md`.

---

## TL;DR — recommendation up front

**Ship a tier ladder, not a big-bang converter.** Users point Holon at an existing
Obsidian or LogSeq vault and it *reads* immediately; write-back is earned per-format,
per-file, and always disclosed.

**Recommended first shippable increment: Tier R/O ingest for LogSeq-Markdown**, because
its outline-per-block model maps 1:1 onto Holon's block substrate — it is the cheapest
big feature-parity win. Immediately after: **LogSeq-Org**, which is *already* ~80% ingestible
through the existing `OrgFormatAdapter` and needs only a thin pre-normalization shim (see §5).
**Obsidian-Markdown ships R/O next** at paragraph-block granularity (free-form prose does not
map to an outline), and stays R/O longest.

**The seam already exists.** `holon_core::file_format::FileFormatAdapter` (parse + render +
`build_block_params` + `check_writeback_lossless`) is exactly the abstraction we need. The
work is: (a) two new adapter impls parsing into the *same* `Block`+`MarkSpan` substrate,
(b) making the vault watcher + `FileSyncController` route *per file* to the right adapter by
extension (today one adapter is bound vault-wide), and (c) a **tier gate** that makes R/O
provably read-only — the write-back path must refuse to touch an R/O-tagged file.

**ADR 0025 is the spine.** A foreign vault is precisely ADR 0025's "external file edits"
intent-less boundary. R/O is the honest floor: no op ever grounds a write to a foreign file,
so per ADR 0025 *no write is ever legal*. Tier PRESERVING later reuses the exact same
`check_writeback_lossless` guard + quarantine machinery already built for org.

---

## 1. First principles

Goal: **zero-conversion onboarding**. A user with a 5,000-note Obsidian vault should get
useful Holon behaviour (navigation, backlinks, search, query, task rollup) within seconds of
pointing Holon at the folder, and should never fear that Holon corrupted their vault.

Constraints / optimization targets, in priority order:
1. **Never lose or mangle the user's data.** This dominates everything. A foreign vault is
   *someone else's source of truth*; Holon is a guest. ADR 0025's fail-loud doctrine applies
   with extra force — for a foreign vault the safe default is *don't write at all*.
2. **Degraded functionality is acceptable and expected**, but must be *disclosed*, never
   silent (CLAUDE.md fail-loud priority ladder: "falls back visibly" beats "silently degrades
   to look fine").
3. **One substrate.** Every format parses into the same `Block` tree with `MarkSpan` marks,
   `tags`, typed `properties`, `task_state`. Downstream (IVM, backlinks, query, render) is
   format-blind. Adapters are the *only* format-aware code. This is already the architecture;
   we extend it, we don't fork it.
4. **Parse, don't validate.** Unsupported constructs become *disclosed opaque/verbatim blocks*
   carrying their exact source bytes — never dropped, never silently reinterpreted. An Obsidian
   callout Holon can't render natively is still a block whose content round-trips verbatim.

The key realization: **read and write are separable and have wildly different risk/cost.**
Reading a foreign format is a bounded parsing problem. Writing it back losslessly requires
byte-preserving anchored rendering (hard) *or* full native-format fidelity (harder). So the
architecture must let a vault be **readable long before it is writable**, and make that state
first-class and visible.

---

## 2. The compatibility model

### 2.1 Per-file format detection + FormatAdapter routing

`FileFormatAdapter` (in `holon-core/src/file_format.rs`) already defines the seam:
`extensions()`, `parse()`, `render_document()`, `render_blocks()`, `doc_id_from_content()`,
`build_block_params()`, `content_differs()`, `sync_document_metadata()`,
`check_writeback_lossless()`. `OrgFormatAdapter` implements it today.

**Gap:** `FileSyncController::with_format` binds *one* `Arc<dyn FileFormatAdapter>` for the
whole vault, and `OrgFileWatcher` hardcodes the `.org` extension filter
(`holon-orgmode/src/file_watcher.rs:87`, `:25-31`). Foreign vaults are heterogeneous (LogSeq
markdown vault can contain both `.md` and `.org`; an Obsidian vault is `.md` + attachments).

**Design:** introduce a `FormatRegistry` — an ordered `Vec<Arc<dyn FileFormatAdapter>>` — and a
resolver `fn adapter_for(path) -> Option<&dyn FileFormatAdapter>` that routes by lowercased
extension. The watcher's extension filter becomes the *union* of all registered adapters'
`extensions()`. The controller looks up the adapter per file at ingest/render time instead of
holding a single `self.format`. This is a mechanical change to a well-isolated field
(`self.format` → `self.formats.adapter_for(path)`), plus a generic watcher that filters on the
registry's extension union rather than a hardcoded `"org"`.

**New adapters** (new crate `holon-markdown`, or two crates `holon-obsidian` / `holon-logseq`
sharing a `holon-markdown-core` — recommend a single `holon-markdown` crate with two adapter
structs to start, split later if they diverge):
- `LogseqMarkdownAdapter` — `extensions() = &["md", "markdown"]`, outline-per-block.
- `ObsidianMarkdownAdapter` — `extensions() = &["md", "markdown"]`, paragraph-per-block.
- LogSeq-org needs **no new adapter** — it's `.org`; it needs a pre-normalization hint (§5).

Because both markdown adapters claim `md`, the registry needs a **vault-flavor discriminator**:
detect `logseq/config.edn` ⇒ LogSeq flavor; detect `.obsidian/` ⇒ Obsidian flavor. This is a
one-time vault classification recorded in vault config, not a per-file guess. A vault with
neither marker defaults to Obsidian-Markdown (the more permissive, prose-oriented parser).

### 2.2 Parsing into the substrate (both formats)

Target types already exist and are sufficient:
- Links → `MarkSpan { InlineMark::Link { target, label } }`.
  - `[[Page]]` / `#tag` / `#[[multi word]]` → `EntityRef::Name { name }` (dangling; lazy
    page-create per links-ruling; resolution lives in `block_links.resolved_id`, content never
    rewritten).
  - `[[Page|alias]]` → `Name { name: "Page" }`, `label = "alias"`.
  - `((uuid))` (LogSeq block ref) → `EntityRef::Internal { id: block:uuid }`.
  - `[text](https://…)` → `EntityRef::External { url }`.
- Emphasis `**b**`/`*i*`/`~~s~~`/`==hl==`/`` `code` `` → `Bold`/`Italic`/`Strike`/(Underline or
  a new `Highlight`? — see open question O3)/`Code` marks.
- LogSeq `property:: value` and Obsidian YAML frontmatter → `Block.properties` (typed) +
  `tags`/`aliases` routed to the tag/alias edge fields exactly like org drawer properties.
- LogSeq `TODO/DOING/DONE/LATER/NOW` + `[#A]` + `SCHEDULED:/DEADLINE:` → `task_state`,
  `priority`, `scheduled`, `deadline` (identical to org — `build_block_params` already emits
  these from a generic `Block`).
- Journals: a file under `journals/` (LogSeq) or matching the Obsidian daily-notes `format`
  (from `.obsidian/daily-notes.json`) is tagged as a journal/date page so the existing journal
  machinery recognizes it.

**Parse-don't-validate for the unsupported tail** — every construct we can't model natively
becomes a **disclosed opaque block**, never a drop:
- Obsidian callouts `> [!note]`, embeds `![[...]]`, comments `%%...%%`, LogSeq
  `{{query ...}}` / `{{embed ...}}` → a block with `content_type` carrying the verbatim source
  and a `_foreign_opaque` internal property naming the construct kind. It renders as a
  disclosed "unsupported: callout" card (degraded-visible, not silent), and on any future
  write-back round-trips its exact bytes. This satisfies ADR 0025 discipline "every block field
  either round-trips or is DECLARED internal" and rule (3)/(4) of the block-loss taxonomy
  (types must carry the full payload; no lossy implicit middle).

### 2.3 Round-trip tiers (explicitly disclosed, per file)

| Tier | Meaning | Write-back | Rendering requirement | Ships |
|---|---|---|---|---|
| **R/O** (read-only mirror) | Holon reads foreign files; Holon-side edits stay in Holon's own store, never touch the foreign file. | **None — provably.** | none | **first** |
| **PRESERVING** | Holon writes back only the blocks it *touched* (op-grounded), byte-preserving every untouched region. | op-grounded, span-anchored | anchored/spans-based partial render | LogSeq-md subset, later |
| **FULL** | Native-format writer; Holon owns the file. | full render each write | complete format fidelity | org today; foreign = far future |

**R/O is the ADR-0025-honest floor.** For a foreign file, *no Holon op ever grounds a write to
that file* (the file wasn't produced by a Holon op; edits to its projected blocks are Holon-store
edits). Therefore, per ADR 0025, **any write to an R/O file is loss by definition and must fail
loud.** R/O isn't a lesser tier we tolerate — it is the *correct* tier until we build anchored
write-back. Concretely R/O = the file is ingested; its doc is flagged `foreign_readonly`; the
write-back path (`on_block_changed` / `re_render_all_tracked` / `materialize_missing_page_files`)
**hard-skips** any doc so flagged, and asserts loudly if ever asked to render one.

**PRESERVING** later reuses the machinery already built for org: `check_writeback_lossless`
(ADR 0025 row-28 guard) + quarantine. The added requirement is *span-anchored partial render*:
we must rewrite only the byte range of the block that changed and leave the rest of the file
untouched (a foreign file has prose/whitespace/constructs we can't regenerate). This needs each
parsed block to carry its **source byte span**; on write-back we splice the re-rendered block
into the original bytes. LogSeq's `- ` outline makes this tractable (each block is a bullet with
clear boundaries). Obsidian free-form prose makes it much harder → Obsidian stays R/O longer.

**FULL** for foreign formats is explicitly *not a near-term goal* and may never be worth it.

### 2.4 Degraded-mode disclosure surface

Two levels, both fail-loud-visible (never silent):
- **Per vault:** on open, a banner/summary — "LogSeq vault, 342 files, Tier R/O. 12 files
  contain unsupported constructs (queries, whiteboards)." Recorded in vault config + surfaced
  in UI.
- **Per file / per block:** each doc carries its tier (`foreign_readonly` / `preserving`) and a
  count of opaque blocks; each opaque block renders as a disclosed "unsupported: <kind>" card.
  A file that failed to parse is **quarantined and named**, not skipped silently.

This is the CLAUDE.md ladder made concrete: (1) works with real data for supported constructs,
(2) falls back visibly for unsupported ones, (3) fails with a clear error for unparseable files,
never (4) silent.

---

## 3. ADR 0025 compliance (the write-back doctrine)

A foreign vault is the "external file edits" intent-less boundary named in ADR 0025. Compliance:
- **R/O:** trivially compliant — no write path is reachable. The guard is a *tier gate* that
  vetoes writes before any rendering, and asserts on violation. The spike proves this with a
  directed test (a mutation against an R/O-tagged doc must produce zero disk writes).
- **PRESERVING:** the existing `check_writeback_lossless` (block-preservation, not byte-equality)
  + `writeback_drops` mass-truncation tripwire + quarantine apply unchanged. Span-anchored render
  is an *additional* fidelity mechanism layered under the same guard, not a replacement.
- **Diff bases are sink truth** (ADR 0025 standing discipline): foreign-file ingest already
  diffs old-parse vs new-parse in `FileSyncController`; nothing about foreign formats changes
  that — we only add adapters, not new diff bases.

---

## 4. What LogSeq-Org needs vs what already works (cheapest big win)

LogSeq-org vaults (`:preferred-format :org`) are `.org` files. The existing `OrgFormatAdapter`
already parses org headlines→blocks, `:PROPERTIES:` drawers→properties, `:id:`→identity,
`TODO/SCHEDULED/DEADLINE`. **So a LogSeq-org vault is ~80% ingestible today with zero new code.**

Deltas to close (thin normalization shim, not a new adapter):
1. **Block-ref token `((uuid))`** appears literally inside org bodies (LogSeq reuses its own
   block-ref syntax even in org mode). The org parser today treats it as plain text. Fix: teach
   the org inline-mark extractor to recognize `((uuid))` → `Internal` link mark (shared with the
   markdown adapters — put it in `holon-markdown-core` or a shared inline util).
2. **`:id:` is lowercase** (LogSeq) vs Holon's `:ID:`. Confirm the drawer parser is
   case-insensitive on the ID key (org spec says drawers are case-insensitive; **verify** —
   flagged as a check in the spike).
3. **Link syntax:** LogSeq writes `[[Page Name]]` (its bracket form) rather than org's
   `[[file:path][desc]]`. Holon's org link resolution already treats a bare `[[target]]` as a
   name/creation intent (ORG_SYNTAX.md) — so this likely already works; **verify** against the
   seeded `Org Compat Test.org` fixture.
4. **File-level `#+title:`** — LogSeq sometimes realizes page title as a property drawer rather
   than a header (issue #11033). Low priority; title is cosmetic for ingest.

**Verdict:** LogSeq-org is the cheapest *first* proof that foreign-vault ingest works, and a
strong candidate to ship *before* the markdown adapters — it's a shim + fixture test, not a new
crate. Recommend the spike covers it explicitly (it does — `Org Compat Test.org` is seeded).

---

## 5. Increment plan (fleet-sized)

Each increment is independently landable, green, and disclosed. Increments 1–3 are R/O only.

- **Inc 0 — Registry + per-file routing seam (mech-executor).** `FormatRegistry`, generic
  extension-union watcher, `FileSyncController` routes per file. Org behaviour unchanged
  (single-adapter registry). Keystone stays green. *De-risks the seam before any format work.*
- **Inc 1 — LogSeq-org shim + fixture (executor).** `((uuid))` inline mark in the org path;
  verify lowercase `:id:` and bare `[[Page]]`; directed fixture-vault ingest test over
  `Org Compat Test.org`. Cheapest parity win.
- **Inc 2 — LogSeq-Markdown R/O adapter (executor).** New `holon-markdown` crate,
  `LogseqMarkdownAdapter`: outline→blocks, `property::`→properties, task markers, `[[ ]]`/`#tag`/
  `((uuid))` marks, journals recognition, opaque blocks for `{{query}}`/`{{embed}}`. Directed
  fixture-vault ingest test. **This is the recommended first *shippable* user-facing increment.**
- **Inc 3 — Obsidian-Markdown R/O adapter (executor).** `ObsidianMarkdownAdapter`: YAML
  frontmatter→properties/tags/aliases, wikilinks/embeds/tags marks, paragraph-block granularity,
  callouts/`%%comments%%`→opaque blocks, daily-notes journal recognition from
  `.obsidian/daily-notes.json`. Directed fixture-vault ingest test.
- **Inc 4 — Tier gate + disclosure surface (executor).** `foreign_readonly` doc flag; write-back
  hard-skip + loud assert; per-vault + per-file disclosure. **Directed test proving R/O never
  writes** (the spike ships an early version of this).
- **Inc 5 — Keystone integration (mech-executor).** Boot the keystone SUT over a seeded foreign
  fixture vault; assert ingest totality (no block loss) reusing `inv-blocks-match-ref`.
- **Inc 6+ (later, needs ruling) — PRESERVING write-back for LogSeq-md subset.** Source-byte
  spans on blocks; span-anchored partial render; reuse `check_writeback_lossless` + quarantine.

---

## 6. Open questions — RULED 2026-07-13

Rulings: **O1** Inc 1 (LogSeq-org shim) then Inc 2 (LogSeq-md); Obsidian is the biggest
userbase but we get there incrementally. **O2** single `holon-markdown` crate — with the
added requirement that dialects are DECLARATIVE: Obsidian-compat and LogSeq-compat are
each one YAML config file over one engine (dialect-as-data), split crates only if they
truly diverge. **O3** new `Highlight` `InlineMark` variant. **O4** read AND write for
both dialects, WITHOUT source-byte spans — see §6a for the ratified design. **O5**
single-vault now, multi-vault long-term; keep the single-vault path free of decisions
that would make multi-root a rewrite.

### 6a. O4 ratified design: convergent canonical form (no byte spans)

Constraints (Martin): files need NOT stay byte-identical to what the foreign app keeps;
what must never happen is (a) LogSeq/Obsidian can no longer read a Holon-written file,
or (b) write ping-pong — each app rewriting the other's output differently, forever.

1. **No-oscillation is a fixed-point property, not a preservation property.** Holon's
   serializer targets the FOREIGN APP'S OWN NORMAL FORM — write what LogSeq/Obsidian
   would themselves write after an edit. Then the foreign app re-saving our file is
   byte-identical (fixed point in ≤1 normalization pass from either side). LogSeq
   already normalizes on edit, so one initial normalization pass is acceptable.
2. **Never write without a semantic change.** Rewrite a file only when parsed content
   (AST) differs; byte diffs never trigger writes. Kills churn on files Holon only read.
3. **Opaque preservation at construct granularity replaces byte spans.** Unmodeled
   syntax (plugin blocks, dataview queries, unknown frontmatter keys) is carried as
   opaque blocks/inline spans and re-emitted verbatim.
4. **Fidelity is testable against the real apps.** Golden corpus of files actually
   written by LogSeq/Obsidian (the two seeded test vaults are the seed); CI asserts
   parse→serialize is byte-stable on the corpus. Feasibility EVALUATED 2026-07-13:
   - **Tier 1 (BUILD, ~0.5-1 day): hermetic mldoc CI oracle.** `mldoc@1.5.9` (LogSeq's
     OWN OCaml parser compiled to JS) runs under node; proven against the real test
     vaults. `parseJson` = parse-fidelity check; `astExportMarkdown(parseJson(x)) == x`
     = the fixed-point oracle for this section's property 1. Already surfaced real
     normalizations (`:LOGBOOK:`→lowercase, embed/query brace forms, tabs-not-spaces,
     fence-info spacing). Vendored node sidecar, no GUI/network. Caveat: mldoc's
     exporter ≈ but ≠ the running app's outliner file-writer — real app keeps residual
     authority.
   - **Tier 2 (CONDITIONAL): presence-gated real-LogSeq golden-refresh script** to close
     the mldoc-vs-real-writer gap; only after Tier 1 flags a disagreement (LogSeq only
     rewrites blocks on EDIT, so oscillation needs a scripted edit).
   - **Tier 3 (DON'T automate): headless Obsidian.** No faithful headless parser exists
     (npm `obsidian` = type stubs; community parsers are approximations that are least
     trustworthy exactly on callouts/embeds/anchors). Obsidian preserves unedited bytes;
     its only real normalization vector is the Properties/frontmatter editor → do a
     ONE-TIME manual golden capture of that and encode it as a static Tier-1 fixture.
   Spike artifacts: /Users/martin/.claude/jobs/ceb646ab/tmp/foreign-app-eval/ (mldoc
   installed + working parse/reserialize driver).

The normal form (bullet char, indent width, property syntax, task keywords, link style,
file placement policy) is exactly what the per-dialect YAML of O2 declares.

### 6b. Original open questions (superseded by the rulings above)

- **O1 — Ship order:** LogSeq-org shim (Inc 1) first, or straight to LogSeq-md (Inc 2)? Inc 1 is
  cheaper and proves the pipeline; Inc 2 is the bigger user-visible win. Recommend Inc 1 then Inc 2.
- **O2 — Crate shape:** single `holon-markdown` (two adapter structs) vs `holon-obsidian` +
  `holon-logseq`. Recommend single crate now; split if they diverge.
- **O3 — `==highlight==`:** map to the existing `Underline` mark (lossy but round-trips as
  underline), add a new `Highlight` `InlineMark` variant (faithful, touches the mark enum +
  every renderer), or opaque-inline. Recommend a new `Highlight` variant — marks-as-truth means
  we shouldn't alias distinct user intent.
- **O4 — PRESERVING appetite:** is byte-preserving write-back to LogSeq-md in scope this quarter,
  or is R/O-forever acceptable for v1 (Holon edits live in Holon; foreign vault is a read mirror)?
  This decides whether blocks must carry source-byte spans from day one.
- **O5 — Multi-vault:** today config is single-vault (`OrgModeConfig.root_directory`). Foreign-vault
  onboarding implies "open this other folder." Is multi-root in scope, or one-vault-at-a-time?

---

## 7. Spike scope (this session)

Read-only ingest of both seeded test vaults, no write-back:
- `LogseqMarkdownAdapter` + `ObsidianMarkdownAdapter` parsing fixtures → blocks with wikilink
  marks, properties, tags, task markers, journals recognized, opaque blocks for unsupported.
- In-repo committed fixture vaults (`crates/holon-markdown/tests/fixtures/`).
- Directed tests: parse fixtures, assert block tree + marks + properties + tag/journal
  recognition + opaque-block disclosure (no drops).
- **Write-guard proof:** a directed test asserting the tier gate refuses to write an R/O doc.
