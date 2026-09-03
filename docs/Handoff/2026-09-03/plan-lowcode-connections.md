# Low-code connections: formats and systems as sidecars, not crates — v2

Planning lane, read-only tree `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/kitchen-dogfood`
at main 89e2efeaa1ff. Every claim carries a path:line or a doc section. v2 answers the senior review
(six amendments, marked ✎).

## 1. First principles

**Goal.** A user attaches Holon to a format or a system they already use by authoring *data*, not
Rust. The only Rust we write is generic and amortised across every connection.

**Constraints.** *Platform-complete* — macOS, Android (GPUI native, no shell, no runtime compiler)
and the `wasm32` web worker. The worker graph already contains the kitchen crate
(`frontends/holon-worker/Cargo.toml:64-74` → `holon`; `crates/holon/Cargo.toml:64` →
`holon-kitchen`, unconditional), while `holon-app` (holding `shopping_rest.rs`) is in neither wasm
graph — which is why the shopping write leg is silently desktop-only today. *Fail loud* — an
unsatisfiable sidecar errors at boot, never degrades to a missing feature. *Parse, don't validate* —
a sidecar becomes a typed value once, at load. *PBT-testable with fakes.* *Secrets never in
sidecars* — only `${VAR}`, and writing `${VAR}` is what **marks** a value secret and strips it from
logs (`crates/holon-mcp-client/src/rest_transport.rs:286`, `assets/integrations/shopping.yaml:31-35`).

**Optimisation targets, in order.** Zero Rust per connection; one generic host per provider *kind*;
minimum total Rust; one mapping language shared by both halves.

## 2. Premise correction: the two halves are not equally bespoke

| Concern | Where | Verdict |
|---|---|---|
| Wire contract (URL, verbs, query, body envelope, cadence, `response_version_path`) | `assets/integrations/shopping.yaml` (74 lines) | **Already declarative**, and more expressive than a UTCP HTTP template |
| Generic REST transport | `crates/holon-mcp-client/src/rest_transport.rs` | Generic, keep |
| Response JSON → domain values | `shopping.rs:262-300`, `:349-424`, `:24-179` | **The real gap** |
| Rows → command envelope | `shopping_sync.rs:31-142` | The same gap, write direction |
| Reconciliation (tombstones, watermark, absence-as-deletion, idempotency) | `shopping.rs:541-657`, `shopping_sync.rs:188-269` | Generic for id-less lists. Keep, rename |
| `.cook` parse → typed rows | `cook.rs` 325, `rows.rs` 132, `file_format.rs` 204 | Bespoke per format. Replace with a plugin |

Both halves are missing the *same* primitive: a declarative mapping from JSON to typed rows. That is
this plan's unifying claim.

**What survives of UTCP: one piece.** The sibling lane validated a real manual against a mock peer
(both legs, idempotent replay) and still recommends against it — no body template, no response
mapping, no declared query parameters, no cadence, `${VAR}` is substitution only so the resolved
secret reaches exception text, and a file-sourced manual silently registers zero tools while
reporting success. Surviving: the **OpenAPI converter, re-cast as an importer** that emits a Holon
sidecar (Inc 7, ~150 lines). Not surviving: the manual as a runtime format, `rs-utcp` 0.3.2 as a
dependency, the call-template shape as our schema. ✎ **D-D is pending** a verifier double-checking
these capability claims against the latest spec and the extension path Martin asked about; if it
finds a response-mapping or body-template extension, only Inc 7's scope changes — no other increment
depends on the answer.

**Axis A (systems)** is therefore: existing sidecar transport + a generic mapping layer + the generic
reconciler + the importer. Only the mapping layer is new.

## 3. Target architecture

### 3.1 The neutral contract: JSON Lines of typed rows

The existing `TypedRowSet` (`crates/holon-core/src/file_format.rs:52-66`) on the wire, so the sink
(`crates/holon/src/core/typed_row_sink.rs:34-41`, built at
`crates/holon/src/api/operation_dispatcher.rs:1643`) is unchanged.

```
{"holon_rows":1,"scopes":[{"type":"recipe","owner_column":"source_path","owner_value":"Rezepte/Pfannkuchen.cook"},
                          {"type":"ingredient_use","owner_column":"recipe_id","owner_value":"recipe:Rezepte/Pfannkuchen.cook"}]}
{"type":"recipe","row":{"id":"Rezepte/Pfannkuchen.cook","title":"Pfannkuchen","servings":"4|6","course":null}}
{"type":"ingredient_use","row":{"id":"…::iu::mehl-0","recipe_id":"recipe:…","raw_name":"Mehl","quantity":250.0,"unit":"g","step_index":0}}
```

Four rules, each a loud error when broken. (1) **Line 1 declares every scope**; a scope with zero
following rows is legal and load-bearing — it is how the last row of a set gets swept, which
inferring scopes from present rows would make unrepresentable. (2) **Replace-scope semantics**, as
today (`file_format.rs:44-51`). (3) **Ids derived from content, never position**
(`file_format.rs:61-64`); the host re-checks with `checked_local_id` promoted out of the kitchen
crate (`rows.rs:115-131`). (4) **Unknown type or owner column refused**, as
`typed_row_sink.rs:60-70` already does. JSON Lines first; CSV/Arrow only on a measured need.

### 3.2 One provider host, not three

**Host 1 — WASM plugin host (primary; the only platform-complete one).** Runtime **`wasmi` 2.0.0**
(2026-09-01, MIT/Apache-2.0): the only runtime that builds for all three targets from one code path
(pure Rust, `no_std`, documented `wasm32-unknown-unknown` build), with a wasmtime-like API so a swap
stays contained. Rejected: **extism 1.30** embeds `wasmtime ^43`, whose `runtime` feature does not
compile to wasm, so the browser needs the separate JS SDK — two hosts, two ABIs; Android is wasmtime
Tier 3 with no CI. **`wasm_runtime_layer` 0.7** is the clean native/browser split but is a
single-maintainer crate; keep as the escape hatch if interpreter speed bites.

ABI: **not** the component model (wasmi has none; no runtime gives components on all three targets).
Four core-wasm functions — `alloc`, `dealloc`, `parse(ptr,len,ctx_ptr,ctx_len)`, `last_error` — JSON
Lines through linear memory, ~100 host lines. Capability model, which also disposes of the WASI
question: **guests are pure functions.** Bytes plus context in, rows out; no filesystem, clock or
network. wasmi's WASI-p1-only limit therefore never binds.

**Host 2 — tree-sitter as a plugin *template*, not a native host.** `tree-sitter` 0.27's runtime
grammar loading needs `wasmtime-c-api` (cannot target wasm32; tree-sitter#4336) and the loader shells
out to a C compiler (dead on Android), so a native tree-sitter host can never be platform-complete.
Instead: one generic guest per grammar, built from a template (`tree-sitter-c2rust` 0.25.2, pure
Rust, wasm32-clean + the grammar + `jaq-core`), with the `.scm` query and jaq filter passed as *data*
at call time. Adding a format = drop a grammar `.wasm` beside a sidecar. Stated cost: a *new* grammar
needs a build step, so grammars are low-code, not no-code.

**Host 3 — CLI, desktop-only,** declared and refused loudly on Android/web so a feature is never
silently missing. Build only if a real connection needs it.

### 3.3 One mapping language: jaq

`jaq-core` 3.1.1 / `jaq-std` 3.0.3 / `jaq-json` 2.0.3 (2026-08-28, MIT). `jaq-core` is `#![no_std]`
with pure-Rust deps; upstream's `jaq-play` uses `web-sys` feature `DedicatedWorkerGlobalScope`, so
jaq already runs in a browser worker. Compile a filter once, run over many values. `jq`'s
`.. | select(…) | {…}` with stream-of-results semantics maps 1:1 onto "walk a document, emit N rows",
and one engine serves four call sites: AST-JSON→rows, response→rows, rows→commands, and the category
vocabulary parse. Rejected: `jsonata-rs` (incomplete, no release in 19 months), `jmespath` (no
arithmetic or recursive descent), `serde_json_path` (selector only), a language of our own.
`jaq-interpret` is dead — do not use it.

**Deliberate divergence from the sibling lane**, which sizes the gap as a 200-300-line field-path
mapper. The deciding evidence is in that same report: the `code_icon_color` vocabulary parser is 156
lines of Rust *with zero constants* (`shopping.rs:24-179`), and rename is del-plus-add. Neither is a
field path. A field-path mapper covers one peer's easy 80% and then needs a Rust escape hatch for
every peer after it — the exact failure mode this plan exists to end. Cost, stated: users must learn
jq, and its errors are terse (D-C2).

### 3.4 Sidecar admission, and where sidecars live

Follow the precedent: bundled copies compiled in
(`crates/holon-mcp-client/src/bundled_sidecars.rs:21-35`) plus user overrides from
`{config_dir}/connections/*.yaml` (`crates/holon-app/src/wiring.rs:379`), behind the existing
`schema_version` generation gate. At boot, one typed load with `deny_unknown_fields`, refusing
loudly on: an extension two adapters claim (`file_format.rs:272-291`); an undeclared type or owner
column; a `${VAR}` with no preference definition (`crates/holon-frontend/src/preferences.rs:227`,
`integration_vars.rs:31-48`); a non-HTTPS non-localhost URL; absent declared exports; a `cli`
provider where no shell exists.

**Write-back is `WriteTier::ReadOnly` by default** (`file_format.rs:242-249`); a sidecar earns
`ReadWrite` only by declaring a reverse export, and `writeback_drops` grounding
(`file_format.rs:226-234`) applies unchanged.

### 3.5 Mapping onto the five layers (Model.md)

Layer 1 already names external APIs and org files as replicas; a connection *is* a replica, with the
plugin or manual producing `diff(base, current)` as rows and the reconciler holding the base. Layers
2-3 are untouched: rows reach Turso only through the one shared `OperationDispatcher` via
`DispatchingTypedRowSink`, so invariant 4 (exactly one writer) survives and no connector becomes a
second writer. Layers 4-5 unchanged.

## 4. Migration: risk elimination first

Formats (Inc 1-3) and systems (Inc 4-5) are independent after Inc 1 and can run in parallel.

**Inc 0 — spikes, no product change.** ✎ Every pass criterion is an **executed guest**, never a
compile.

| Spike | Pass criterion (evidence artifact) |
|---|---|
| 0.1a macOS | A trivial echo guest returns its input from a native test |
| 0.1b **Android** | `cd frontends/gpui && just deploy` (justfile:86 = build → `apk` → `install` → `launch`; `apk` at :56-57 delegates to `android/build-apk.sh`, which was itself a dogfood finding fixed 2026-09-01, entry `2026-09-02-just-apk-recipe-cannot-build-a-launchable-apk.md`), then `just log` (:77-78, `adb logcat -v brief -s "holon-gpui:*"`). **Evidence = a guest-emitted line in the logcat capture**, teed to a file. A green `cargo check` is NOT a pass |
| 0.1c **Web worker** | `just check-worker-wasm` (Justfile:641) only typechecks, and the wasm target has **no test harness** — no `wasm_bindgen_test` or `wasm-pack test` exists anywhere (Justfile:640 says so). The run path is `frontends/dioxus-web/serve.mjs --build` (README:11-17) driven by a Playwright spec modelled on `frontends/dioxus-web/tests/worker-smoke.spec.mjs:16-37`, which boots real Chromium and captures worker `console`/`pageerror`; guest stderr surfaces through `frontends/holon-worker/web/wasm-log.mjs`. **Evidence = a Playwright assertion on a guest-emitted console line.** CI precedent: `.github/workflows/devex-gates.yml:189-221` |
| 0.2 cooklang guest | `cooklang` 0.18.7 (zero C deps) builds to a `wasm32` guest; record `.wasm` size against a stated budget |
| 0.3 jaq embedded | A compiled filter runs inside the worker build; record filter-compile and per-document time |
| 0.4 tree-sitter guest | `tree-sitter-c2rust` 0.25.2 + `tree-sitter-cooklang` (stale: last commit 2024-05, ABI 0.22) build to one guest; the regeneration cost is the finding |

If 0.1 fails, fall back to `wasm_runtime_layer`; if that also fails, to native-only plugins with the
worker importing rows — escalate to Martin before proceeding.

**Inc 1 — the neutral contract, no plugins.** `holon-rows` (envelope, parser, emitter); route the
*existing* `CookFormatAdapter` rows through serialize→deserialize. Red-first differential PBT against
`recipe_row_sets` on the fixtures and on generated recipes. Kills "does the contract carry
everything" at zero platform risk.

**Inc 2 — the generic host + first plugin.** `PluginFormatAdapter` implements `FileFormatAdapter`,
registered from a sidecar rather than from `crates/holon-app/src/wiring.rs:354`. First guest is
cooklang. Red-first differential PBT: plugin rows ≡ `cook.rs` rows on every fixture and on generated
recipes.

**✎ The divergence rule for every differential PBT (Inc 2 and Inc 4).** "New ≡ old" would silently
bless upstream-versus-ours parse differences — the upstream `cooklang` crate very likely accepts the
German timer units `cook.rs` refuses, which is dogfood entry
`2026-09-02-a-german-timer-unit-refuses-the-whole-recipe.md`. So: **every divergence is NAMED and
triaged as a bugfunnel entry** (old wrong / new wrong / both wrong), never allowlisted silently. The
differential passes when the divergence set is empty, or when every member is an entry carrying a
ruling. A divergence discovered and quietly absorbed is a gate failure, not a pass.

**Inc 3 — delete.** Remove `cook.rs`, `rows.rs`, `file_format.rs` and the `cooklang` dependency from
`holon-kitchen`; promote `checked_local_id`. No old path stays.

**Inc 4 — the mapping layer for systems.** jaq filters for response→rows and rows→commands.
Differential PBT against current semantics, replaying the sibling lane's `mock_peer.py` contract:
version conflict, `command_id` idempotency on replay, absence-as-deletion. Absorbs
`shopping.rs:210-454` and the vocabulary parser (~400 lines).

**Inc 5 — generalise and delete.** `ShoppingReconciler` → `RemoteListReconciler`; per the directive
this is a *move*, so `shopping_reconcile.rs` (445) and `shopping_sync_pbt.rs` (540) move with it.
Delete the API-shaped remainder of `shopping.rs`, `shopping_sync.rs`, `shopping_rest.rs`. Fixes the
platform hole: the write leg leaves `holon-app` and enters the wasm graph.

✎ **Keyed versus content-keyed rows.** The reconciler is generic only for *id-less* lists: identity
is `(name, cat)` because the peer issues no id (`shopping.rs:182-191`). Systems with server ids
(Todoist, JIRA) need keyed rows, and a content-key reconciler would treat a server-side rename as a
delete-plus-add and lose local state. Resolution: **the sidecar declares the key derivation as a jaq
expression**, and `RemoteListReconciler` takes it as a parameter — a server-id key is the expression
`.id`, a content key is `[.name,.cat]`. Both then share one tombstone/watermark path. If that proves
larger than Inc 5 can hold, **Inc 5 scopes explicitly to id-less lists** and keyed rows become Inc 5b;
the plan does not pretend one reconciler falls out for free.

**Inc 6 — the tree-sitter grammar-plugin template**, the no-code format path. Acceptance: a second
format nobody wrote Rust for.

**Inc 7, optional — the OpenAPI→sidecar importer** (~150 lines) and the CLI host if needed.

**Out of scope:** component model; vault-level sidecars; plugin signing/marketplace; CSV/Arrow;
replacing the `rest` YAML with UTCP at runtime; iOS.

## 5. Risks and staleness guard

| Risk | Kill criterion | Inc |
|---|---|---|
| wasmi too slow | Full `Rezepte` scan against `cook.rs`; the 200 ms p95 interaction→projection SLO is the line | 2 |
| ✎ **jaq per-row cost at scale** | Response→rows over a **10k-item list**; same 200 ms SLO as the kill line. Measure filter-compile once and per-row separately — a filter recompiled per row would be the defect, not jaq | 4 |
| `.wasm` guests bloat the APK and worker bundle | Size budget stated in 0.2 before any guest ships | 0 |
| `tree-sitter-cooklang` stale, ABI 0.22 | 0.4 regenerates it or the grammar path defers | 0 |
| `tree-sitter-c2rust` one maintainer, two versions behind | Vendorable; only Inc 6 depends on it | 6 |
| Deleting the kitchen crate loses the acceptance workload | Run §6's ten entries before and after Inc 3 and Inc 5 | 3, 5 |
| Live shopping API base unknown (the stored secret is the human share link, a different host) | Inc 4 validates against the mock; the live leg stays blocked until Martin supplies the API base | 4 |

**Staleness greps per increment:** `rg -n "holon-kitchen" crates/holon/Cargo.toml frontends/holon-worker/Cargo.toml`;
`rg -n "TypedRowSet|TypedRowSink" crates/`; `rg -n "CookFormatAdapter|register_kitchen_types" crates/`;
`ls docs/Testing/bugfunnel/entries/ | grep 2026-09-02`; `just check-worker-wasm`.

## 6. ✎ Acceptance workload: the ten kitchen dogfood entries

From chain commit `f4f41861` (`xtxkksxs`), `docs/Testing/bugfunnel/entries/2026-09-02-*`. Each names
the increment that must turn it FIXED. All ten are `status: OPEN`.

| Entry | Gap | Turned FIXED by |
|---|---|---|
| `a-cook-recipe-loses-its-title-and-all-its-metadata` | COVERAGE | **Inc 1** contract (title/metadata become declared columns, not a false premise about the document block's properties) then **Inc 2** plugin |
| `a-german-timer-unit-refuses-the-whole-recipe` | COVERAGE | **Inc 2** plugin — the upstream crate is expected to accept it; the divergence rule above forces this to be *named*, not silently absorbed |
| `a-refused-cook-file-still-leaves-a-document-block` | COVERAGE+ENVIRONMENT | **Inc 2** host atomicity: a refused parse must emit no scopes and no document block |
| `a-shopping-item-can-never-be-added-in-holon` | COVERAGE | **Inc 5** generic types (`shopping_item` needs a `properties` overflow column for the `_provenance` stamp) |
| `deleting-a-shopping-item-is-undone-by-the-next-sync` | COVERAGE | **Inc 5** — generic `delete` must tombstone, not hard-delete, or the reconciler cannot push the deletion |
| `re-render-all-tracked-renders-read-only-cook-files` | COVERAGE | the **ingest-contract lane** (write-tier gate), not this plan |
| `the-degraded-toast-is-stale-and-calls-cook-files-org` | — | the **ingest-contract lane** |
| `org-write-back-halves-bold-markers-around-a-link` | — | the **org-bold-link lane** |
| `projection-misses-the-slo-on-the-real-vault` | ENVIRONMENT | measured, not fixed, in **Inc 2 and Inc 4** — it is this plan's kill line, and Inc 2 must not make 295-335 ms worse |
| `the-screenshot-tool-returns-a-stale-frame-silently` | — | tooling; outside this plan |

**✎ Dependency on the three parallel lanes — flagged, because I could not verify it.** `ingest-contract`,
`sync-peer-types` and `org-bold-link` have **zero occurrences anywhere in the repo** (grepped both
trees across `docs/`, `.claude/`, `crates/`, `devlog/`, both spellings). I am treating their scope as
told to me, not as established. On that basis: **Inc 5 depends on `sync-peer-types`** for the two
shopping entries above — if that lane adds the `properties` overflow column and tombstoning delete to
the generic type machinery, Inc 5 consumes it; if it does not, Inc 5 must do it and grows. **Inc 2
depends on `ingest-contract`** only for the two write-tier/toast entries, which Inc 2 must not
regress. **`org-bold-link` is independent.** Before Inc 2 and Inc 5 start, confirm each lane's landed
scope against the tree rather than against this table.

## 7. Testing

Dedicated PBTs share the keystone catalog, generators and SUT so promotion stays a move. The generic
host gets a PBT over **fake plugins** — one that echoes, one emitting a malformed envelope, one that
traps, one returning an unknown type — asserting each failure is a named `Err`, never a silent skip.
The generic connector gets a PBT over **fake manuals** driven by the mock peer contract. Differential
PBTs under the §4 divergence rule are the spine of Inc 2 and Inc 4, and are what make the Inc 3 and
Inc 5 deletions safe. End to end, the ten entries above are the gate; `dogfood-explorer` is final.

## 8. Decision cards for Martin

- **D-A Runtime.** wasmi 2.0 vs `wasm_runtime_layer` vs extism. *Recommend wasmi*; revisit on 0.1/Inc 2
  measurements. Con: an order of magnitude slower than a JIT on compute-bound guests.
- **D-B tree-sitter.** Plugin template vs native runtime loading. *Recommend the template.* Con: not
  no-code for a new grammar.
- **D-C Mapping language.** jaq vs our own. *Recommend jaq.* Con: users must learn jq; terse errors.
- **D-C2 Mapper size.** jaq vs the sibling lane's 200-300-line field-path mapper. *Recommend jaq*, on
  the 156-constant-free-line vocabulary parser a field path cannot express. Con: a second embedded
  language in the tree.
- **D-D UTCP.** *Recommend import-only.* ✎ **Pending** the verifier's re-check of the capability
  claims and the extension path; only Inc 7's scope turns on it.
- **✎ D-E Sidecar location.** (a) **Config dir only** — `$HOME/.config/holon` on macOS
  (`crates/holon-frontend/src/config.rs:364-387`), `/data/data/space.holon.gpui/files/config` on
  Android (`frontends/gpui/src/mobile.rs:83-93`). Con, and it is sharper than v1 admitted: with
  own-device pairing (D68.b, whole-store replication) a config-dir sidecar **does not reach the
  phone**, because replication travels per-container through Loro docs and plain config files sit
  outside every container — so every connection must be re-authored per device. (b) **Vault-level**
  replicates, but grants vault content the power to load code and open network connections, which is
  a privilege escalation the vault must not have. (c) **In the store, as a device-local-by-default
  container with an explicit "replicate this connection" flag**, secrets staying per-device via
  `${VAR}` prefs. *Recommend (c)*, with (a) as the Inc 1-5 interim so no increment blocks on it. Cons
  of (c): it needs the device-local container machinery D77 is deciding right now, so it inherits
  that decision; and a replicated connection means a plugin binary crossing devices, which reopens
  D-F on a device that never consented to that plugin. Note D68.b lives only in code comments
  (`crates/holon-integration-tests/src/pbt/composed/two_instance.rs:395-400`,
  `tests/two_instance_composed_pbt.rs:1241-1289`), not in `docs/` — worth landing as a doc.
- **D-F Plugin trust.** Pure-function guests with no ambient capability keep this low-stakes, but
  distribution and provenance are unresolved and gate Inc 6 — and D-E option (c) couples to it.
