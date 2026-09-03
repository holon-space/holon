# Verify — lane `ingest-contract` — **REFUTED**

Fresh-context adversarial pass. WS `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/ingest-contract`,
uncommitted diff on `89e2efea` (18 files, 1057+/68-). All evidence produced in this session.

## Verdict

**REFUTED** — one behavioural defect (D1) and one teeth gap (D2).
Fixes 1 (partially), 2, 3 and rev-2 R1/R2 are otherwise confirmed; every claimed gate reproduced.

---

## D1 — DEFECT: removed document metadata goes stale (fix 1)

`holon_core::apply_document_metadata` (`crates/holon-core/src/file_format.rs`) only
INSERTS and UPDATES. A key the parsed document no longer carries is never removed, so
the stored document block keeps metadata the file does not have.

Shape: a metadata-bearing file ingested with `servings` + `source`, then re-ingested
after the user deleted `source` from it.

Expected `doc.get_property_str("source") == None`; actual `Some("Familienrezept")`:

```
the property the user DELETED from the file is still on the document block —
the store shows metadata the file does not have.
Props: {"source": String("Familienrezept"), "title": String("Spaghetti Carbonara"), "servings": String("4")}
  left: Some("Familienrezept")  right: None
```

Contradicts the lane report's "a parsed document's property bag lands verbatim on the
persisted document block". It is also ASYMMETRIC with org's own override
(`crates/holon-orgmode/src/file_format.rs:175-188`), which explicitly handles the
`None` (removal) case for `FILE_ID_KEYWORD` — so the removal case was known at the
seam and not carried into the generic default. Priority order 4 ("silently degrades
to look fine") applies: the UI shows stale metadata with no disclosure.

Log: `lane-logs/verify-scratch-01.log`. Driven through the real dispatcher
(`FileSyncController::on_file_changed`), not the pure function.

## D2 — TEETH GAP: the FILE-keying of fix 4 is unpinned

Inverting the production keying — `disclosure.ingest_refused(path, …)` →
`ingest_refused(Path::new(adapter.format_name()), …)`, i.e. keying by FORMAT instead
of FILE — leaves `a_refusal_names_its_format_and_is_retracted_when_the_file_ingests`
GREEN (`lane-logs/verify-teeth-fix4.log`, 2 tests run, 2 passed).

Cause: the test asserts `refusal.contains("Spaghetti-Carbonara.fixture")` against a
`DisclosureLog` string that folds `reason` in, and `reason` is `format!("{e:#}")`,
whose `with_context` wrapper already embeds the path. The assertion is satisfied by
the reason, never by the subject.

A subject-only probe (records `path` alone) goes RED under the same inversion —
`Subjects: ["fixture", "fixture"]` (`lane-logs/verify-teeth-fix4b-INVERTED.log`) — and
green once restored (`lane-logs/verify-teeth-fix4b-RESTORED.log`). The prod code is
correct; nothing in the suite would notice if it stopped being.

Consequence if it regresses unnoticed: one refused file's repair clears/replaces every
other refused file's banner — exactly the staleness the entry was filed against.

---

## Confirmed

**Gates, re-run by me (not trusted from the report):**

| Gate | Result | Log |
|---|---|---|
| `lane-logs/gate.sh` (fmt + check + arch + crates) | fmt: no churn; `cargo check --workspace --all-targets`: 0 errors; arch **7/7**; crates **590 passed, 1 skipped** | `lane-logs/verify-gate-01.log` |
| `lane-logs/finish-gpui.sh` | `cargo check -p holon-gpui --all-targets` clean; **16 tests run: 16 passed** | `lane-logs/verify-gpui-01.log` |
| `just keystone-smoke` | `4 passed; 0 failed`, EXIT=0 | `lane-logs/verify-keystone-smoke.log` |

**Teeth, fix 2 (atomicity) — inverted by me.** The hoisted `let new_parse = …` moved
back below the document resolve/mint. RED for the right reason:

```
a REFUSED file left a document block behind: ["Spaghetti-Carbonara"] — the sidebar
grows an empty page for a file nothing was read from, and every later write-back
trips the tier gate over it
```
Log: `lane-logs/verify-teeth-fix2.log`.

**Adversarial shapes (scratch tests, since deleted):**

- **Refused → fixed → broken again**: raise / clear / raise, and the second raise
  carries the CURRENT reason, not the stale first one. PASS.
- **Two files of one format, one refused**: only the bad file raises; the healthy
  file's ingest retracts nothing belonging to the bad one. PASS.
- **Read-only format file edited on disk**: file bytes byte-identical after
  `on_file_changed` + `re_render_all_tracked`, and the edit reached the projection.
  PASS.
- **Metadata removed on re-ingest**: FAIL — see D1.

**Generic-ness (check 5).** `rg -ni 'cook|kitchen|shopping'` over every changed
non-test file: zero control-flow hits. Every match is a doc-comment example, the
`holon-kitchen` adapter naming itself, pre-existing registry wiring in `wiring.rs`, or
GPUI test-fixture data.

**Rev-2 `.expect(..)` (check 6) — unreachable, as claimed.** `FormatRegistry::require`
is a pure extension→adapter lookup over an immutable registry
(`crates/holon-core/src/file_format.rs:351`), and the only caller that reaches the
error arm (`on_file_changed`) got its error from an `ingest_file` that resolved the
same adapter for the same path. The one path that could carry an unregistered
extension into `on_file_changed` is `on_file_renamed`'s `on_file_changed(to)`, and the
watcher admits `Renamed { from, to }` only when `is_relevant(&to)` holds
(`crates/holon-orgmode/src/file_watcher.rs:105-113`), so `to` always has a registered
extension.

**Bus semantics.** `DegradedSignalBus::clear` broadcasts only when the condition was in
effect (`crates/holon-loro/src/degraded_signal_bus.rs:307-316`), so the unconditional
`ingest_recovered` on every successful ingest is not a broadcast storm.

---

## Notes (not defects)

1. **Subject keying uses the raw path, internal state uses `CanonicalPath`.**
   `ingest_refused`/`ingest_recovered` key by `path.display()`
   (`crates/holon-app/src/loro_seams.rs:651-668`) while `quarantined`,
   `last_projection` and `readonly_rerender_skipped` all key by `CanonicalPath`. Two
   spellings of one file would raise a banner its own repair cannot clear. Not shown
   reachable — the controller's callers pass one spelling — but the asymmetry is
   latent.
2. **`on_file_renamed` is `pub` and does not guard `to`'s extension itself.** Its
   safety rests entirely on the watcher's filter. A future non-watcher caller (an
   MCP-driven rename, a test) reaches the rev-2 `.expect` and panics instead of
   getting `require`'s loud error.
3. The task brief expected `ingest_recovered` in `crates/holon-core`; it is in
   `crates/holon-filesystem/src/sync_ports.rs:491`. Brief inaccuracy, not a defect.

## Restore integrity

Every file I inverted was restored by `cp` from a pre-run backup and checked by
`shasum -a 256` (never `jj restore`):

```
8e1a13d993a9677b2fe57e6aaf9fb14d768b005809785ff8687d693b46568478  crates/holon-orgmode/tests/ingest_contract.rs
24886c99b805c82bc9f0ded6aa38f16469b0b312e2849ed6e993f36c9fd567f5  crates/holon-filesystem/src/file_sync_controller.rs
15fc49910d4fe79fe46f4d629859c7eb0607d9221f1099ea2cc9f561157dee04  crates/holon-core/src/file_format.rs
```

Identical to the pre-run baseline. `jj diff --stat` back to 18 files, 1057 insertions,
68 deletions. All scratch tests deleted. No jj/git write command was run.

---

# Rev 3 — delta verdict: **CONFIRMED**

Only the delta re-verified. All evidence produced in this session; every inverted
file restored by `cp` + `shasum -a 256` (never `jj restore`), scratch tests deleted.

## D1 — CLOSED

`apply_document_metadata` now reconciles the persisted bag to exactly
`parsed.properties` + the declared title: it deletes every persisted key the parse
does not declare, then upserts the declared set
(`crates/holon-core/src/file_format.rs:122-151`).

Probes, all driven through the real `FileSyncController::on_file_changed`
(`lane-logs/verify-rev3-shapes.log`, 4 tests run, 4 passed):

- **My original refuting probe, re-driven verbatim** — delete `source` from the
  file, re-ingest: `get_property_str("source") == None`, `servings` intact. PASS
  (this is the exact assertion that produced the rev-2 REFUTED).
- **Second shape, property RENAMED** (`source` → `origin`, same value): old key
  gone AND new key present, `servings` and the declared title untouched. PASS —
  this is the shape a delete-only or an add-only reconcile would each fail one
  half of.
- The lane's own `re_ingest_removes_metadata_the_file_no_longer_declares`
  reproduces green independently.

Entry `docs/Testing/bugfunnel/entries/2026-09-03-deleted-file-metadata-survives-re-ingest.md`
is present.

## D2 — CLOSED, now with teeth

The subject-only assertion is real: `SubjectLog::subject_names()` records the
`path` alone and is compared with `assert_eq!` against the exact refused file, so
the reason string can no longer satisfy it
(`crates/holon-orgmode/tests/ingest_contract.rs:920-935`).

Repeating my rev-2 inversion — `ingest_refused(path, …)` →
`ingest_refused(Path::new(adapter.format_name()), …)` — now goes RED for the right
reason (`lane-logs/verify-rev3-d2-INVERTED.log`):

```
the refusal is not keyed by the file it is about, so one file's repair clears
every other refused file's banner
  left: ["fixture"]
  right: [".../Spaghetti-Carbonara.fixture"]
```

Restored, green.

## Org override parity (check 3)

The override is kept for the three things the default does not do (doc-root body,
`todo_keywords`, file drawer), and its removal semantics AGREE with the default's.
Driven directly against `OrgFormatAdapter::sync_document_metadata`: a file-level
drawer key the file no longer declares is gone (the whole drawer is replaced), a
still-declared key survives, and a dropped `#+ID:` marker is removed via the
override's explicit `None` arm. PASS.

So both legs now treat the file as the authority in BOTH directions; there is no
format where a deleted key survives a re-ingest.

## Gate

`lane-logs/gate.sh` once, `-j6` under the `holon-build` semaphore →
`lane-logs/verify-rev3-gate.log`: fmt no churn, `cargo check --workspace
--all-targets` 0 errors, arch **7/7**, crates **592 passed, 1 skipped** (up from
590 — the two new tests).

## Restore integrity

```
3e3e3aa43da9466cb6106d9b6978a36a5e6a2067000aac41f45dcc2e9d61bed7  crates/holon-orgmode/tests/ingest_contract.rs
24886c99b805c82bc9f0ded6aa38f16469b0b312e2849ed6e993f36c9fd567f5  crates/holon-filesystem/src/file_sync_controller.rs
47e4dca3db1b0ce81fce886665edf475e2faca834d279828f8803d67d838e2f4  crates/holon-core/src/file_format.rs
```
Identical to the pre-run rev-3 baseline.

## Carried-over note (unchanged by rev 3)

Rev 2 note 1 still stands: `ingest_refused`/`ingest_recovered` key by raw
`path.display()` while all controller-internal state keys by `CanonicalPath`. Not
shown reachable; latent.

New, low severity: the reconcile now deletes any persisted doc-block property the
FORMAT does not declare. Correct for a read-only authoritative format. For a
read-write format using the default (LogSeq / Obsidian markdown), a property added
to a page through the UI is dropped on the next re-ingest unless the renderer
writes it to disk first. Not probed — flagging the seam, not claiming a defect.
