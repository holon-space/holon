# ADR 0011: Abstract disk access behind a `FileSystem` port (+ `FileChangeSource` watch seam)

Status: Accepted (with review amendments, see "Resolved design decisions")
Date: 2026-06-10

**Implementation note (2026-06):** The ports were implemented in `crates/holon-filesystem/` (see `src/fs_port.rs` for `FileSystem` / `RealFileSystem`, `src/change_source.rs` for `FileChangeSource` / `NotifyWatcher`, `src/in_memory.rs` for `InMemoryFileSystem`) rather than inside `holon-orgmode` as the original ADR text implied. `holon-orgmode` depends on `holon-filesystem` and wires the adapters via DI. The legacy `directory.rs` `DataSource` still lives in `holon-filesystem` and drags a `holon` dependency — cleanup pending.

> This ADR is self-contained: it records every fact an implementer needs
> (motivation, the exact call-site inventory, trait surface, phasing, and
> validation gates). It does not assume access to the investigation that
> produced it.

## Summary

Introduce two injected ports in the `holon-orgmode` crate so that all
filesystem interaction goes through traits instead of direct `tokio::fs` /
`std::fs` / `notify` calls:

1. **`FileSystem`** — read/write/metadata/path operations ("where the bytes live").
2. **`FileChangeSource`** — the file-change notification seam ("how I learn a file changed").

Production binds them to a real-disk adapter (`RealFileSystem` + a
`notify::RecommendedWatcher`-backed change source), behaviourally identical to
today. Tests bind them to an **in-memory** adapter whose `write` emits the
change event **synchronously on close**, making the org-file sync path
deterministic and removing the per-write wait latency.

This is the standard hexagonal/ports-and-adapters move and mirrors seams
already in this codebase (ADR 0004 domain-adapter/actor split, the
`BlockQuerySource` read seam, the backend-blind `FileSyncController`).

## Context

### How an org-file change reaches storage today

The org-file → storage sync path is:

```
write .org file
  → notify::RecommendedWatcher (OS watcher: fsevents on macOS)
  → OrgFileWatcher maps notify Event → EventKind::Modify|Create|Remove
       (crates/holon-orgmode/src/file_watcher.rs:132)
  → SHA256 content-change gate (content_changed; known_hashes map)
       (file_watcher.rs:86,177; CanonicalPath-keyed)
  → FileSyncController::on_file_changed(path)
       (crates/holon-orgmode/src/file_sync_controller.rs:275)
  → RE-READS the file BY PATH: tokio::fs::read_to_string(path)
       (file_sync_controller.rs:277, 1076, 1181, 1211, 1288, 1356, 1433)
  → parse org → project to SQL / Loro
```

The consumer **re-reads the file from disk by path** — it does not receive the
content in the event. This matters for the design: a pure event bus is
insufficient; the change signal and the byte source must be coherent.

### Why this path is slow and flaky

The OS watcher is asynchronous, coalesced, debounced, and has **no
write-boundary semantics** (no "file closed / write complete" signal). The
system compensates with several latency/normalisation layers:

- **500 ms debounce** on the watcher (`OrgModeConfig.debounce_ms = 500`,
  crates/holon-orgmode/src/di.rs:635, defaults at 651 and 663).
- **`notify::watch(RecursiveMode::Recursive)` can take 9+ seconds to arm on
  macOS** (documented workaround at di.rs:111, 1117, 1196; armed at 1222).
- **Test-side polling waits** that exist purely to absorb the above:
  - `TestEnvironment::wait_for_org_file_sync` — polls until a content hash
    matches, **5 s timeout** (crates/holon-integration-tests/src/test_environment.rs:2245).
  - `wait_for_org_files_stable` — mtime-stability poll.
  - `poll_org_file_mtime_stable` — mtime granularity poll.
- A **known false 5 s timeout**: the sync-completion hash heuristic times out
  even when the body is already correct (admitted in-code at
  test_environment.rs:2270–2272: "hash produces 5 s timeouts in cases where the
  body is already [correct] … this hash is just a sync-completion heuristic").

The root cause of the flakiness is the **partial-write window**: because
fsevents fires without a close boundary, a reader can observe a half-written
file, producing a hash mismatch that the polling layers paper over.

### Measured cost

In `general_e2e_pbt` (the `sql_only` slice, `PROPTEST_CASES=2`), the
file-writing transition (`ApplyMutation`, External/file variant) cost **~3.6 s
per call (~10 s across 3 calls)** — almost entirely these waits — and one call
hit the full 5 s `wait_for_org_file_sync` timeout. This is the single largest
non-floor offender remaining after the per-transition quiescence windows were
tuned. (Other transitions settle at a ~25–50 ms quiescence floor; the org-file
round-trip is an order of magnitude worse and is the reason to act.)

### The disk-access surface is small and contained

All org-file disk interaction lives in **5 files of one crate**
(`holon-orgmode`); `holon-app` and `holon-frontend` do not touch org files on
disk. Inventory (non-test, current tree):

| File | sites | operations |
|---|---|---|
| `file_sync_controller.rs` | 26 | `read_to_string`×7, `write`×4, `create_dir_all`×4, `exists`×4, `canonicalize`×3, `read`(bytes)×1, `metadata`×1, `modified()`×1, `notify::watch`×1 |
| `file_watcher.rs` | 10 | `write`×4, `create_dir_all`×2, `recommended_watcher`/`notify::watch`/`.watch()`/`Error`, `exists`×1 |
| `di.rs` | 9 | `canonicalize`×4, `notify::watch`×2 / `.watch()`×2 / `Watcher` / `RecursiveMode`, `read_to_string`×1 |
| `orgmode_sync_provider.rs` | 8 | `canonicalize`×4, `read_to_string`×2, `write`×1, `exists`×1 |
| `file_utils.rs` | 1 | `read`(bytes)×1 |

Aggregate operation set the port must cover: `read_to_string` (~10),
`read` bytes (~2, images), `write` (~9), `create_dir_all` (~6),
`canonicalize` (~11 — the most common; used to normalise hash-map keys),
`exists` (~6), `metadata`/`modified` mtime (~2), plus the `notify` watcher
across 3 files. (`read_dir`/`remove_file` may appear in adjacent helpers; grep
before finalising the trait — see "Implementation notes".)

There is already a `CanonicalPath` newtype used as the watcher's hash key
(file_watcher.rs:86–87) — reuse it for path normalisation in the in-memory FS.

## Decision

### Two ports (not one)

```rust
// "where the bytes live"
#[async_trait::async_trait]
pub trait FileSystem: Send + Sync {
    async fn read_to_string(&self, p: &Path) -> std::io::Result<String>;
    async fn read(&self, p: &Path) -> std::io::Result<Vec<u8>>;        // images
    async fn write(&self, p: &Path, contents: &[u8]) -> std::io::Result<()>;
    async fn create_dir_all(&self, p: &Path) -> std::io::Result<()>;
    async fn remove_file(&self, p: &Path) -> std::io::Result<()>;
    /// Recursive directory walk respecting .gitignore — the port-level form of
    /// `scan_directory` (file_watcher.rs), which is the codebase's single
    /// source of truth for directory walking. (Replaces the earlier `read_dir`
    /// sketch: no call site uses raw `read_dir`.)
    async fn scan_directory(&self, root: &Path) -> std::io::Result<ScannedEntries>;
    async fn metadata(&self, p: &Path) -> std::io::Result<FileMeta>;   // carries mtime
    fn exists(&self, p: &Path) -> bool;
    fn canonicalize(&self, p: &Path) -> std::io::Result<PathBuf>;
}

// "how I learn a file changed" — the hook seam
pub struct FileChange { pub path: PathBuf, pub kind: FileChangeKind } // Modify|Create|Remove
pub trait FileChangeSource: Send + Sync {
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<FileChange>;
}
```

Keep them separate: `FileSystem` is about bytes; `FileChangeSource` is about
notifications. The real adapters are independent (`tokio::fs` vs `notify`); the
in-memory adapter implements *both* and couples them so a `write` fires a
`FileChange` synchronously (see below).

### Adapters

- **`RealFileSystem`** — thin passthrough to `tokio::fs` / `std::fs`. Must be
  byte-for-byte behaviour-preserving vs today.
- **`NotifyWatcher`** — wraps the existing `notify::RecommendedWatcher` setup
  (the di.rs arming logic, debounce, 9 s-recursive-watch workaround) behind
  `FileChangeSource`.
- **`InMemoryFileSystem`** — `Mutex<BTreeMap<CanonicalPath, Vec<u8>>>` + a
  monotonic logical mtime counter + a `broadcast::Sender<FileChange>`.
  `write(p, bytes)` updates the map **and synchronously sends the `FileChange`**
  on the broadcast channel (the "close" hook); `read_to_string(p)` returns the
  committed bytes from the map. Because the write is atomic-from-the-map's
  perspective and the event fires only after the full write, there is **no
  partial-write window, no debounce, no mtime poll** — and the false 5 s hash
  timeout cannot occur.

### Wiring

`FileSyncController`, `OrgFileWatcher`, `orgmode_sync_provider`, and the
`holon-orgmode` `di.rs` factory take `Arc<dyn FileSystem>` and
`Arc<dyn FileChangeSource>` via the existing `fluxdi` DI, instead of calling
`tokio::fs::*` / constructing the watcher directly. Production registers the
real adapters; the PBT harness registers the in-memory adapters.

With the in-memory change source, the PBT settle waits
`wait_for_org_file_sync` / `wait_for_org_files_stable` /
`poll_org_file_mtime_stable` are **replaced by a deterministic await on "the
`FileChange` for this write has been processed by `on_file_changed`"** (e.g. a
completion signal the controller emits per processed change, or a seq the
harness waits on).

## Phasing (risk is front-loaded into nothing)

**P1 — `FileSystem` port, pure refactor, zero behaviour change.**
Define `FileSystem` + `RealFileSystem`; route the ~72 `fs::*` sites in the 5
files through an injected `Arc<dyn FileSystem>`. Production stays byte-identical.
*Gate:* the full `holon-orgmode` test suite + the org-touching integration
tests stay green. This is the clean-design win on its own, with no behavioural
risk — it is mostly volume (mechanical substitution + threading the dependency
through constructors), not difficulty.

**P2 — `FileChangeSource` port.**
Define the trait; move the `notify` watcher behind `NotifyWatcher`; production
binds it. Still no behaviour change. *Gate:* same suites green; the real
watcher path is unchanged.

**P3 — in-memory adapter + delete the waits (the payoff).**
Add `InMemoryFileSystem` (implementing both ports), wire the PBT harness to it,
and replace the org-file sync waits with the deterministic hook await. *Gate:*
`general_e2e_pbt` slices green; the ~10 s `ApplyMutation` cost and the false
5 s timeout are gone; org-file sync is deterministic. Keep **one small
dedicated test** that still drives the real `NotifyWatcher` so the fsevents
integration (debounce, coalescing, recursive-watch arming) remains covered.

P1 and P2 are safe, reviewable, behaviour-preserving. P3 is where the
speed/determinism lands, isolated.

## Consequences

Positive:
- Disk access becomes an injected dependency — the design improvement stands
  on its own (testability, no hidden global I/O, matches the repo's DI/seam
  idiom).
- The org-file sync path becomes **deterministic** under test (no fsevents
  debounce/coalescing/partial-write), removing a known flake source — valuable
  for property-based testing specifically.
- Removes the largest non-floor PBT offender (~10 s) and the false 5 s timeout
  at the root rather than papering over it.

Costs / risks:
- A mechanical ~72-site pass plus DI threading through constructors in 5 files.
  Low conceptual risk, real volume.
- `RealFileSystem` must be a faithful passthrough; the gate is the existing org
  suite staying green (sizeable, so review the diff for any behavioural drift —
  e.g. error-kind mapping, `canonicalize` semantics on non-existent paths).
- The in-memory test path no longer exercises real fsevents; **mitigation:** the
  retained dedicated `NotifyWatcher` test. This honours the repo rule "keep
  prod and the E2E test similar" — one faithful path is kept; the bulk is made
  fast and deterministic.

## Scope

- Confined to `holon-orgmode` (plus the test harness wiring in
  `holon-integration-tests`).
- **Production stays on real disk.** External editors writing `.org` files do
  not go through our `FileSystem`, so the in-memory FS is strictly a test
  adapter; production must keep the real watcher + debounce + atomic-write
  handling for that external-writer reality.

## Alternatives considered

1. **tmpfs / in-memory file *contents* still watched by fsevents.** Rejected:
   disk I/O is not the bottleneck (org files are tiny, already on SSD);
   fsevents watches tmpfs with the same debounce and coalescing latency. Moves
   bytes to RAM and still pays the watcher.
2. **Change-source seam only (keep real temp files, but fire the change event
   in-process after the test's own write; leave reads on real disk).** A lighter
   variant capturing ~80 % of the win for ~20 % of the work — the test's
   `write` rings the bell directly, and the subsequent real `read_to_string`
   sees the just-written bytes. Viable, but leaves a split/partial FS
   abstraction. The full port was chosen for cleaner design and total
   determinism; if effort must be minimised, this is the fallback and is
   compatible with P2 (implement `FileChangeSource` first, defer the in-memory
   `FileSystem`).
3. **Point fixes without a port:** lower `debounce_ms` for the test config
   (di.rs:635) and fix the SHA hash heuristic's false 5 s timeout
   (test_environment.rs:2270). Cheap partial mitigations that do not remove the
   async/partial-write nondeterminism and do not improve the design; can be done
   independently as stopgaps.
4. **Do nothing.** Keep the ~10 s cost and the flaky 5 s-timeout path.

## Resolved design decisions (review, 2026-06-10)

Settled in review before implementation; these amend the sketch above where
they conflict.

1. **Trait home = the existing `holon-filesystem` crate** (not a new
   `holon-orgmode::fs` module). It already exists, already holds the
   file/directory domain entities, and is already a dependency of
   `holon-orgmode` and `holon-frontend`. The ports will be shared by **all
   file-based SerDe implementations** (Org today, Markdown later) — anything
   that would otherwise be duplicated across them belongs behind these traits.
   All three adapters (`RealFileSystem`, `NotifyWatcher`, `InMemoryFileSystem`)
   live there too (`notify` becomes a `holon-filesystem` dependency;
   `InMemoryFileSystem` is a normal pub item so the test harness can wire it).

2. **`scan_directory` is a port op; `read_dir` is not.** No call site uses raw
   `read_dir`; the real enumeration is `scan_directory()`
   (file_watcher.rs:28–55, `ignore::WalkBuilder`, gitignore-aware). The
   in-memory impl is a prefix scan over the path map (no gitignore needed in
   tests).

3. **Canonicalization happens *inside* the trait implementation.** Call sites
   stop calling `std::fs::canonicalize` themselves; they pass paths as-is and
   the adapter normalises (real: `std::fs::canonicalize` incl. macOS
   `/var → /private/var`; in-memory: lexical normalisation). The port keeps an
   explicit `canonicalize` for the places that need the normalised form as a
   hash-map key (`CanonicalPath`), with real-fs error parity: error on
   non-existent paths (file_sync_controller.rs:1600–1606 relies on the
   fallback-to-parent dance). Exception: the config-time canonicalize calls in
   `di.rs:646–659` run before DI exists and stay on `std::fs` — the port scopes
   post-construction I/O.

4. **`FileChangeSource` sits *below* `OrgFileWatcher`'s SHA-256 content gate.**
   The port emits raw `FileChange`s; `OrgFileWatcher` keeps the hash gate and
   consumes the port instead of constructing `notify` directly. This keeps
   dedupe/echo-suppression identical between prod and tests — essential because
   `FileSyncController` writes files itself (write-back at
   file_sync_controller.rs:1226, 1448): with a synchronous in-memory
   `write → FileChange`, the controller's own writes echo back as change events
   and must be filtered by the same gate that filters them in prod.
   (`broadcast::send` is non-blocking, so no deadlock risk.)

5. **Events fire only on complete writes.** The trait's `write` is
   whole-buffer — there is deliberately no streaming/`File`-handle API — so the
   end of the `write` call *is* the close boundary. The in-memory adapter sends
   the `FileChange` synchronously after the full content is committed to the
   map; a partial-write window is unrepresentable.

6. **Corrected inventory: ~30 non-test sites, not ~72.** The original table
   counted `#[cfg(test)]` code (e.g. all four `file_watcher.rs` writes are in
   tests). Verified: `holon-app`/`holon-frontend` fs usage is
   config/logging/theme only — no org-file disk access. The deliberate
   `read_to_string().unwrap_or_default()` baseline/TOCTOU sites
   (file_sync_controller.rs:1181, 1211, 1433) keep their semantics; the
   `di.rs:1149` page-cache warmup is real-fs-only and is skipped for the
   in-memory adapter.

## As-built notes (implementation, 2026-06-10)

All three phases landed. Deviations / discoveries vs the plan above:

- **The `OrgFileWatcher` SHA-256 hash gate was dead code.** Production used
  `new_unarmed → into_parts` and discarded the hash map;
  `content_changed`/`update_hash` had no callers. The real echo suppression is
  `FileSyncController::last_projection`. Decision 4's "keep the hash gate" was
  therefore moot — the gate was deleted and `OrgFileWatcher` is now a thin
  filter bridge (`.org` extension + gitignore) from `FileChangeSource` to the
  sync loop's mpsc channel.
- **Pre-existing bug fixed:** the watcher's gitignore filter used
  `Gitignore::matched`, which never consults parent dirs — a `vendor/` pattern
  did not ignore `vendor/dep.org` (test `test_file_watcher_respects_gitignore`
  failed on the base commit). Now `matched_path_or_any_parents`.
- **`DEFAULT_ASSETS` empty-vault seeding moved behind the port:** from eager
  `std::fs` in `holon-app::wiring` to `OrgModeConfig.seed_assets` +
  `seed_default_org_assets` in the OrgModeModule factory (before the initial
  sync task and controller scan). Required because the harness overrides the
  fs binding in `extra_setup`, which runs after `add_frontend`.
- **DI binding contract:** `OrgModeModule` default-binds `dyn FileSystem` /
  `dyn FileChangeSource` (real adapters) via `try_provide`; the test harness
  replaces them with one shared `InMemoryFileSystem` via `override_provider`
  in `extra_setup` (before any resolution).
- **`CanonicalPath` interplay:** `CanonicalPath::new` consults the real fs and
  falls back to the input path. The harness therefore canonicalizes the temp
  dir once (`TestEnvironment::org_root`) and builds every in-memory org path
  from it, so fallback keys strip_prefix cleanly against the controller root.
- **Waits:** `wait_for_org_file_sync` (the false-5s content-hash heuristic) is
  deleted; `await_org_file_convergence` now waits only on the event-driven
  `OrgSyncIdleSignal` quiescence (+ logical-mtime sanity poll). The 100 ms
  post-write watcher-detection sleep in `TestEnvironment::write_org_file` is
  gone. `debounce_ms` turned out to have no consumer (dead config).
- **Retained real-watcher coverage:** `holon-filesystem` change_source test +
  the three `OrgFileWatcher` tests drive the real `NotifyWatcher` (fsevents)
  end to end.
- Known follow-up: the MCP `read_org_file` / debug tools still read the real
  disk directly (`frontends/mcp/src/tools.rs`); unused by PBT, but live
  inspection of an in-memory-fs session would need them routed through the
  port.

## Implementation notes (for the executing session)

- `async_trait` is already a workspace dependency.
- Before finalising the trait, re-grep the 5 files for any operation not in the
  inventory above (`read_dir`, `remove_dir_all`, `File::open/create`,
  `symlink`, `set_*`), and grep `holon-app` / `holon-frontend` to confirm they
  remain free of org-file disk access.
- Reuse the existing `CanonicalPath` newtype (file_watcher.rs:86–87) for
  in-memory map keys; `canonicalize` is the most-used op (~11 sites) and must
  behave consistently for non-existent paths (real `std::fs::canonicalize`
  errors on missing files — preserve that, or define the port's contract
  explicitly).
- Home for the traits: `holon-filesystem` (decision 1 above).
- Implementation happens in the `pbt-tracing-investigation` worktree (clean at
  start; decided in review — supersedes the original fresh-worktree advice).
