# Stage-1 plan — read-only LogSeq-DB import into Holon

Lane: `lsqdb-import` (jj workspace on main 84648cd9). Scope: **read-only import**
of a LogSeq DB-version graph into Holon. Ruled GO by Martin 2026-08-21 on the
stage-0 spike. Sources: `~/.claude/plans/logseq-db-spike-2026-08-20/REPORT.md`,
`~/.claude/plans/logseq-db-storage-research-2026-08-20.md`,
`docs/Architecture/Model.md`.

Base sentinel OK (`crates/holon-integration-tests/tests/fixtures/logseq-parity/`
exists, 16 feature files). All paths absolute; `pwd` printed beside every gate
verdict; subagent Bash cwd resets to primary → never rely on relative paths.

---

## 1. First principles (what stage 1 must do, and must not)

**Goal.** Turn a LogSeq DB `db.sqlite` snapshot into Holon `Block`s in the store,
losslessly on the spine (uuid, parent, order, title, tags, timestamps, standard
properties), with **disclosed** opaque carriers for everything un-modeled, and
**loud failure** on anything the mapping cannot classify. Never silently drop.

**Hard constraints (ruled, non-negotiable):**
- READ-ONLY. No code path writes a LogSeq db file, ever, this stage.
- Acceptance gates on the **identity check**, not regex counts: dedup by the full
  `(e,a,v,tx)` 4-tuple is stable, and `#(:block/uuid datoms) == #(uuid-bearing
  entities)`. The regex lower-bound cross-check is unsound (spike-proven) — drop it.
- Fail loud: a datom/attr the mapping can't classify goes into the typed
  `_logseq_raw/*` carrier or errors — never dropped. Nested EDN values carry as
  opaque raw.
- Holon invariants: order is minted by the consolidator (inv 2); `set_field("sort_key")`
  is a hard error (inv 3); do not adopt LogSeq's fracdex keyspace — **re-mint**
  order on import (inv 10). Page identity is name-chain of page ancestors; pages
  nest only under pages (`Model.md:144-170`) — do not relax.

**Out of scope (explicit):** the write leg (stage 3, API-driven); one-way live
follow / op-log tail (stage 2); CDC watching; any UI / `frontends/gpui` change;
`shadow_builders`; a real Holon schema layer for typed properties/classes
(research Q1 → recommendation (b): import typed props as untyped JSON + opaque
carrier, accept lossy export deferred to stage 3). **Touches neither
`frontends/gpui` nor `shadow_builders`** — no conflict with the table-widget or
journals lanes.

---

## 2. Where the code lives

**New crate `crates/holon-logseq-db`** — a Layer-1 replica *adapter*, sibling
of `holon-org-format` (research §2: "a LogSeq-DB backend is a new Layer-1 replica
adapter, a sibling of holon-org-format"). Justification against the existing
layout (`ast-outline digest`): `holon-org-format` is already a standalone crate
whose only job is text↔`Block`. A LogSeq adapter has its own heavy, isolated
deps (a SQLite reader + a Transit-JSON subset decoder) that belong nowhere near
the org crate. A new crate keeps those deps contained and the adapter
independently testable.

**Dependencies:**
- `holon-api` — `Block`, `EntityUri`, `BlockContent`, `Tags`, `Value`, `ContentType`.
- A read-only SQLite reader: **`libsql` (0.9)**, already a workspace dep and
  standard-sqlite compatible — opens the LogSeq `db.sqlite` copy read-only
  (`SELECT addr, content FROM kvs`). (Alternative: the vendored `turso` crate;
  libsql chosen for plain on-disk-sqlite read fidelity. Decide at Inc 2; if
  either can't open an external file cleanly, add `rusqlite` — flagged.)
- `serde_json` — the Transit doc is JSON; the decoder walks `serde_json::Value`.
- Store-entry wiring (Inc 4) needs `holon-core` (`BlockOrdering`). Kept behind a
  thin `ingest` module so the pure decode/projection path stays `holon-api`-only
  and unit-testable in isolation.

**Do NOT implement `FileFormatAdapter`** (`holon-core/src/file_format.rs:60`):
its `parse(content: &str)` / `render_*` contract is text-and-path oriented and
render is write-back — out of scope. Provide a dedicated `LogseqDbImporter`
instead.

---

## 3. SerDe design (parse-don't-validate)

Pipeline: `db.sqlite → kvs rows → Transit decode → leaf-datom iterator →
dedup → typed LogseqDatom → group by entity → classify → Block projection`.

**Newtypes / enums (parse at the boundary, illegal states unrepresentable):**
- `Eid(i64)` — the datom `e` slot (LogSeq entity id).
- `Tx(i64)`.
- `LogseqAttr` — enum of the mapped `:block/*` / `:logseq.*` attributes the
  projection understands, plus `Raw(String)` for schema-declared-but-unmapped
  attrs (→ `_logseq_raw/*` carrier). An attribute **absent from the schema node
  (addr 0)** is a loud `Err`, not a `Raw`.
- `TransitValue` — decoded value: `Str | Int(i64) | Float(f64) | Bool | Nil |
  Keyword(String) | Symbol(String) | Uuid(String) | Instant(String) |
  Ref(Eid) | Coll(serde_json::Value /* opaque nested EDN */)`.
- `LogseqDatom { e: Eid, a: LogseqAttr, v: TransitValue, tx: Tx }`.

**Transit reader** — a direct Rust port of the spike's Python `_Reader`
(`transit_decode.py`, proven byte-exact on the fixture): map marker `"^ "`,
keyword `~:`, symbol `~$`, uuid `~u`, int `~i`/`~n`, float `~d`/`~f`, instant
`~t`/`~m`, escapes `~~ ~^ ~\``, tagged `["~#tag", v]`, and the **per-document
write cache** (`^0`..`^zz`, `BASE_CHAR_INDEX=48`, `CACHE_CODE_DIGITS=44`,
`MIN_SIZE_CACHEABLE=4`) reset on every top-level decode. Unknown ground-type
prefix → loud `Err` (matches the Python `raise ValueError`). The `^N` back-ref
cache is the #1 silent-corruption hazard (§7 of research) → covered by the
decode round-trip PBT.

**Dedup:** collect leaf datoms across all `kvs` rows with `addr > 0`, dedup on
`(e, a, v, tx)`. The 3 index trees (EAV/AEV/AVE) replicate each datom ~3.12×;
dedup neither over- nor under-counts (a datom is unique by all four slots).

**Attribute vocabulary / schema node:** parse addr 0 for the declared attribute
set + `:branching-factor`; use it to distinguish `Raw` (declared, unmapped)
from loud-error (undeclared).

---

## 4. Datom → Block projection

Group deduped datoms by `Eid`. Classify each entity:
- **kv singleton** (`:logseq.kv/*`, uuid-less) → config, not a block; excluded
  from the 206 (the 9 singletons). Recorded, not dropped.
- **block** (carries `:block/uuid`) → project to `Block`.
- **schema entity** (class/property definition, `:logseq.class/*` /
  `:logseq.property/*` as an entity) → stage-1 treatment: carry as a `Block`
  with its datoms in `_logseq_raw/*` (research recommendation (b)); do **not**
  synthesize a Holon schema layer.

**Field mapping (research §2 table):**
| Block field | LogSeq datom | Notes |
|---|---|---|
| `id: EntityUri` | `:block/uuid` | `EntityUri::block(uuid)` — scheme added at boundary |
| `parent_id` | `:block/parent` (Ref) | root sentinel when absent |
| `tags: Tags` | `:block/tags` → `:logseq.class/*` | class ident → tag name; `Journal`/`Page`/`Task` etc.; `PAGE_TAG="Page"` marks pages |
| `content` | `:block/title` | |
| `content_type`/`source_language` | `:logseq.class/Code-block` + lang prop | code blocks |
| `properties` (JSON) | `:user.property/*` + typed props | untyped JSON this stage |
| `created_at`/`updated_at` | `:block/created-at`/`-updated-at` | both Unix millis — clean |
| `collapsed` | `:block/collapsed?` | |
| — (dropped, Holon re-derives) | `:block/refs` | redundant on import |
| `_logseq_raw/<attr>` | any `Raw` attr, nested EDN | opaque carrier, disclosed |

**Order:** sort each parent's children by their `:block/order` fracdex **string**
to get the intended sibling sequence, then **re-mint** via `place_all` (§5) — do
not store LogSeq's fracdex.

**Journals:** `:block/journal-day` (integer `YYYYMMDD`) → a page tagged
`Journal` + a day property; no dedicated Holon day-key this stage.

**Page/namespace model:** flat pages (`:block/name`) → `Block` tagged `Page`.
Namespace pages `a/b/c` → Holon page-under-page chain is a genuine semantic
translation and Holon **refuses a page under a non-page** (`Model.md:159`).
HolonTest is a fresh graph (0 user classes) and likely has no namespace pages;
stage-1 handles flat pages and emits a **loud `Err` on any `/`-bearing page
name** (fail-loud, not silent flattening) — full namespace-chain construction is
flagged as a fast-follow increment if the corpus needs it.

---

## 5. How import enters the store

Through the **`BlockOrdering` trait** (`holon-core/src/block_ordering.rs`) — the
same sanctioned boundary org re-ingest uses, not the org param-builder (which
carries org-only `_drawer_order`/`file_properties` and a text `StorageEntity`):
1. Build `Vec<BlockCreateRequest>` (parent_id, id, `BlockContent`, properties,
   `BlockEdges` for tags/class) from the projected `Block`s.
2. `BlockOrdering::create_in_tree_batch(requests)` to create the nodes.
3. Per parent, `BlockOrdering::place_all(parent_id, ordered_ids)` to realize the
   **total** sibling order — the consolidator mints a fresh gap-free key
   sequence (its doc-comment names exactly "the org re-ingest case"). This
   satisfies inv 2/3/10: we state order, the owner mints it.

The pure decode+projection (`db.sqlite → Vec<Block> + per-parent ordered ids`)
is `holon-api`-only and unit-tested against the fixture; the `ingest` module
(the two `BlockOrdering` calls) is exercised by the keystone against a real
store.

---

## 6. Keystone / PBT strategy (holon-feature: red-first)

Two layers; **red-for-the-right-reason logged before green** each increment, red
log in the PR.

**(A) Decode-level PBT** (in-crate, `holon-logseq-db`): proptest generates
Transit-JSON documents (maps, keywords, nested colls, cache-eligible repeated
scalars) → property: `decode` is total on valid input and **round-trips**
(decode→re-encode→decode stable), and **dedup is idempotent**. This is the guard
on the `^N` back-ref hazard. In-memory generators, no committed file. Red first
because the decoder doesn't exist.

**(B) Keystone integration test** (`crates/holon-integration-tests/tests/`,
following the existing `logseq_org_vault_ingests_without_loss` pattern):
`holontest_db_imports_with_identity_gate` — imports the committed HolonTest
fixture (§7) and asserts:
- exactly **206** blocks projected (excluding the 9 kv singletons);
- **identity gate:** `#(:block/uuid datoms) == #(uuid-bearing entities) == 206`;
  unique-datom count `== 2631`; distinct attrs `== 58`; distinct entities `== 215`;
- **3 spot-checks** (REPORT §Obj1b): Project Alpha page (e207: name "project
  alpha", title "Project Alpha", tagged Page, uuid `6a86cf74-…`, refs present);
  journal days (e193→20260820, e196→20260819, e204→20260822, tagged Journal);
  the empty-titled task (e203: `:logseq.property/status` → `status.done`,
  deadline `1787349600000`, nested icon coll carried opaque);
- **sibling-order spot-check:** a known parent's children come out in
  fracdex-sorted order after re-mint (guards the "re-mint changed sequence"
  silent-loss spot).

Red first: the test won't compile / imports zero because `LogseqDbImporter`
doesn't exist. This is the keystone for the feature.

**Not extending the composed keystone PBT** (`general_e2e_composed_pbt.rs`): it
drives UI *interactions* over a running SUT; a boundary import transform is a
poor fit for its transition alphabet. Seeding the composed keystone from an
imported LogSeq graph is a plausible *later* integration (stage 2 territory) —
flagged, out of stage-1 scope.

---

## 7. Fixture strategy

**Commit the HolonTest `db.sqlite` copy** as
`crates/holon-integration-tests/tests/fixtures/logseq-db/holontest.sqlite`
(471 KB) + a `README.md` documenting provenance, schema-version `{65,33}`, and
the exact expected numbers (2631/215/206/58) the keystone asserts. Located via
`concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/logseq-db/holontest.sqlite")`
(the established fixture pattern).

**Justification (hermetic, deterministic, safe):** the identity gate and all 3
spot-checks are defined against *this exact* graph — a synthetic fixture cannot
reproduce e203/e207/e193 or the 2631/206 counts. HolonTest is a **fresh
throwaway graph, 0 user classes, 0 user properties** (REPORT §Obj1) — its only
content is test scaffolding (a "Project Alpha" page, Aug-2026 journal days, a
probe task), so committing it carries no personal data. 471 KB is acceptable for
a keystone-class binary fixture. **Gate before commit (REVISED — the original wording was too weak; see A10):**
sweep the RAW BYTES of the file, not the rows:
`strings -a <fixture> | grep -iE "gmail|<surname>|[a-z0-9._%+-]+@[a-z0-9.-]+"`,
and review every hit. A title dump structurally cannot see PII held in a
property *value*, and querying the table cannot see a survivor sitting in page
slack. Any redaction must be followed by `VACUUM` and then re-swept. If
sensitive content remains, fall back to trimming (below). The 471 KB copy referenced from an ignored
path is explicitly **not** acceptable as the only input — it must be in-repo.

A hand-built synthetic kvs fixture is rejected as the *primary* input: correctly
hand-authoring valid Transit-JSON across 3 index trees + a B+-tree with
consistent separators is high-effort and high-risk (tests green, prod broken).
The decode PBT (A) covers shape-space instead. If Martin rejects the 471 KB
binary, fallback is a **trimmed real kvs** (drop AEV/AVE nodes, keep EAV + addr
0) — but that weakens the dedup-identity assertion, so full copy is preferred.

---

## 8. Increments (each independently landable)

- **Inc 0 — scaffold + red keystone.** New crate skeleton; commit fixture +
  README (after the sensitive-content gate); the RED keystone (B) + a RED
  decode-PBT (A) that fail because `holon-logseq-db` is empty. Capture red logs.
- **Inc 1 — Transit reader.** Port the Python `_Reader`; decode-PBT (A) green.
  Loud `Err` on unknown ground type.
- **Inc 2 — kvs → deduped datoms.** SQLite read (libsql) + leaf-datom iterator +
  4-tuple dedup + `LogseqAttr` classification against addr-0 schema. Assert
  2631 unique / 58 attrs / 215 entities on the fixture (pure, no store).
- **Inc 3 — datom → Block projection.** Entity grouping, block/kv/schema
  classification, field mapping, `_logseq_raw/*` carriers, journal + flat-page
  handling, loud `Err` on `/`-page-names. Keystone (B) goes green through the
  projection level (206 + identity + spot-checks, in memory).
- **Inc 4 — store entry.** `ingest` module: `BlockCreateRequest` build →
  `create_in_tree_batch` → `place_all` (fracdex-sorted, re-minted). Full
  keystone green: 206 blocks land in a real store, projections render, sibling
  order correct. Fresh-context `verifier` pass before reporting done.

---

## 9. Risk register

1. **`^N` back-ref cache mis-resolves → wrong attribute silently.** Highest.
   Mitigation: exact port of proven Python cache semantics + decode round-trip
   PBT (A) + the identity gate.
2. **Fixture contains sensitive data.** Mitigation: raw-byte `strings` sweep
   before commit + `VACUUM` + re-sweep (§7, revised); HolonTest is throwaway per
   REPORT. This risk MATERIALIZED — see A10; the original title-dump gate did
   not catch it.
3. **`create_in_tree_batch`/`place_all` won't cleanly carry class/tag edges for
   page/journal classification.** Mitigation: Inc 4 against a real store +
   verifier.
4. **Namespace page-chain not covered by fresh fixture → prod graphs with
   namespaces break.** Mitigation: loud `Err` on `/`-page-names this stage (no
   silent flatten); fast-follow increment if needed.
5. **471 KB binary fixture rejected.** Mitigation: trimmed-EAV fallback (weakens
   identity assertion — flagged).
6. **Order re-mint changes sibling sequence silently.** Mitigation: sibling-order
   spot-check in the keystone (§6 B), asserted against the fracdex sequence both
   at the store boundary and in the store itself.
8. **Sibling order depends on A8 max-tx resolution — a block that MOVED carries
   two `:block/order` datoms. RESIDUAL risk, not covered risk.** Existence
   witness in the corpus: **e201** (child of e200) holds `:block/order` `'a2'`
   at tx 536870962 and `'a0'` at tx 536870965 — the only entity in the fixture
   with more than one distinct order datom. BUT e201 is an ONLY CHILD, so a
   mis-resolved order yields the same one-element sibling sequence: NO fixture
   assertion would catch a wrong resolution. The guard is A8 itself plus the
   `cardinality_one_resolves_to_the_highest_transaction` unit test — not the
   keystone. (The take-first tooth reddening ingest_ordering broke max-tx for
   every cardinality-one attribute at once; it is not evidence that e201's
   order specifically is pinned.) This is what makes A8 a correctness
   requirement for ORDER and not only for timestamps; the fracdex sort is only
   as right as the value feeding it. (Supersedes an earlier duplicate-order-key
   note, withdrawn: it came from not resolving `:block/order` by max tx — i.e.
   from e201 itself — not from a grouping error; no parent's resolved sibling
   group holds a duplicate key.)
7. **libsql can't open the external LogSeq sqlite read-only.** Mitigation: try
   `turso`; else add `rusqlite`. Decide at Inc 2.

---

## 9b. Amendments — resolutions measured against the fixture

A1–A5 were ruled before implementation. A6–A9 below were forced by what the
committed fixture actually contains, measured with the stage-0 spike's own
Python decoder and then reproduced independently by the Rust. All four were
approved by Martin (via team-lead) on 2026-08-21. **Where these disagree with
§§1–8 above, these win** — the sections above describe the plan as ruled, this
section describes what landed.

**A6 — the entity partition is three-way, not two-way.** §4 assumed every
uuid-less entity is a `:logseq.kv/*` config singleton and the keystone asserted
9 of them. The fixture refutes it: 9 entities lack `:block/uuid`, but only 7
carry a `:logseq.kv/*` `:db/ident`. The other two — e197 (`:block/created-at`
plus an empty `:block/title`) and e199 (`:block/created-at` alone) — are
LogSeq's own half-created remnants, carrying no identity and so not
projectable. Folding them in with the config singletons would be exactly the
silent misclassification the fail-loud rule exists to prevent, so they became a
third recorded kind, `EntityKind::Orphan`. `KV_SINGLETONS` is now 7 with
`ORPHAN_ENTITIES` 2, and the keystone's `== 9` count is replaced by a
**totality assertion** — `blocks + singletons + orphans == distinct_entities` —
which is strictly stronger, because it makes it impossible for any entity to be
classified into nothing. The identity gate is untouched: 206 uuid datoms == 206
uuid-bearing entities.

**A7 — the `:db/` meta-vocabulary counts as declared.** §3 ruled that an
attribute absent from the addr-0 schema node is a loud `Err` rather than a
`Raw`. Applied literally that makes this healthy fixture fail to import: three
observed attributes — `:db/valueType`, `:db/cardinality`, `:db/index` — are
never listed inside `:schema`, because they are DataScript's own
meta-attributes *describing* the schema, and they appear as datoms on
property-definition entities. The `:db/` namespace is therefore treated as
declared-by-the-engine. The relaxation is deliberately narrow: it is one
namespace, it is definitionally schema-level, and every user-space attribute
(`:user.property/*`, `:logseq.*`) still raises `ImportError::UnknownAttr` when
undeclared, so the guard keeps its teeth where vocabulary drift actually
happens.

**A8 — cardinality-one resolves by MAX TX.** Not an option; it is what
DataScript means. A cardinality-one attribute can carry SEVERAL datoms, one per
transaction that changed it, and the current value is the highest-tx one. The
fixture proves it: e193 holds `:block/updated-at` at tx 536870916 →
1787218310305 and at tx 536871019 → 1787221153038, with `:block/updated-at`
declared `:db.cardinality/one`. The projection resolves cardinality-one by max
tx and keeps every value for cardinality-many, both driven by `:db/cardinality`
read from the same schema node that supplies `:db/valueType`. This failure mode
is invisible — the superseded value equals the block's `created-at`, so getting
it wrong yields a perfectly plausible `updated_at == created_at` — so the
keystone pins the winning value by name rather than trusting the spot-checks to
notice.

**A9 — epoch 0 for a missing timestamp is a chosen sentinel.** This is the ONE
deliberate exception to the error-over-sentinel rule, recorded so a future
reader does not "fix" it into `now()`. A block with no `:block/created-at` /
`:block/updated-at` datom gets 0, not the import time: a fabricated timestamp is
indistinguishable from a real one, while 1970 is visibly absent. Erroring
instead was rejected because it would block importing otherwise-valid built-in
entities.

**A10 — a logical redaction is not a byte redaction.** Adversarial verification
refuted the fixture's privacy claim: after the in-place row edit, all 456 `kvs`
rows read clean and `PRAGMA integrity_check` returned `ok`, yet four copies of
the original personal email survived in the SLACK SPACE of allocated b-tree
pages (stale page images the in-place write left behind), recoverable with
`strings -a`. Fixed by `VACUUM`, which rewrites the file without slack;
re-verified at 0 occurrences with the identity counts unchanged. Two durable
consequences: the §7 pre-commit gate is now a raw-byte `strings` sweep for PII
patterns rather than a title-dump eyeball (a title dump cannot see a property
*value*, and a table query cannot see page slack), and any future redaction of a
binary fixture must be followed by `VACUUM` and re-swept.

**A11 — `((…))` carries node references only.** The `((` form is restricted to
uuid inners; a non-uuid inner is NOT a link. The `Name` fallback that `[[…]]`
keeps would, on `((`, turn ordinary parenthesised prose into a fabricated
reference — verification demonstrated `((rate*(x)))` producing a link to a page
named `rate*(x` with a mis-measured span. That is manufactured graph data, which
the fail-loud rule exists to prevent. `[[…]]` keeps the name form, because it
legitimately addresses pages by name as well as nodes by uuid.

Two further findings that changed no ruling but are worth carrying: `libsql`
does support a read-only open (`OpenFlags::SQLITE_OPEN_READ_ONLY`), so risk 7
is closed and the no-write rule is enforced by SQLite itself rather than by
convention; and `create_in_tree_batch` creates in request order, so the ingest
must re-sequence the batch parents-before-children — neither LogSeq's entity ids
nor the projection's uuid sort do that, and a block unreachable from any root
raises `ImportError::UnreachableBlocks` rather than being quietly created at the
root.

---

## 10. Staleness guard

Re-run the base sentinel (logseq-parity fixtures dir exists) before each
increment. Absolute paths only; `pwd` printed beside every gate verdict
(subagent Bash cwd resets to primary). Gates run through
`/Users/martin/.claude/skills/orchestrator/scripts/with-build-slot.sh`; quote
runner `test result:` lines from per-run logs (never wrapper exits); `tee`
everything.
