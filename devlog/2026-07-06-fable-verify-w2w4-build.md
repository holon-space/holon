# Verify W2 (hakari traversal-excludes) + W4 (holon-markdown deletion) — adversarial build audit

Date: 2026-07-06. Role: skeptical build/release engineer. Read-only audit of main
(tip `4b8880080b`). Goal: try to BREAK the W2/W4 changes.

## Verdict

Both changes are **sound**. Core claims verified empirically. Three real but
non-blocking findings: one pre-existing feature leak of the same class W2 targeted
(tokio `test-util` + opentelemetry `testing` in prod graphs), two robustness gaps
in the regression test, plus minor W4 debris (empty dir, stale CONTEXT.md row).

## W2 — hakari traversal-excludes (gpui, collections, http_client)

### Verified solid

- `cargo tree -p <pkg> -e normal -i <dep>` for pkg in {holon-tui, holon-mcp,
  holon-turso, holon-loro, holon-filesystem} x dep in {proptest, gpui, wgpu, naga,
  collections, http_client}: **all absent**. Absence shows as either
  `warning: nothing to print.` (dep in dev graph only, e.g. holon-tui/proptest,
  holon-loro/proptest) or `error: package ID specification ... did not match any
  packages` (dep nowhere in that package's resolved graph). wgpu/naga never appear
  because the zed-fork gpui renders via Metal, not wgpu — nothing hidden there.
- `cargo hakari verify` → `info: workspace-hack works correctly`, exit 0.
  The committed workspace-hack/Cargo.toml is canonical for the current config.
- Dropping `collections`/`http_client` from workspace-hack broke nothing:
  `cargo check -p holon-tui -p holon-mcp -p holon-turso` → exit 0 (5m51s, real
  recompile, not cache-trivial). holon-gpui still gets both crates via its own
  `gpui` dependency edge (confirmed in lock: they remain as deps of gpui itself).
- Package names in the test are correct: `holon-tui` = frontends/tui/Cargo.toml:2,
  `holon-mcp` = frontends/mcp/Cargo.toml:2.
- The test runs in CI: `.github/workflows/ci.yml:102` runs
  `cargo test --workspace --exclude rust_lib_holon`, which includes
  holon-architecture-tests. Ran it locally: `no_proptest_in_prod_graph ... ok`.
- **Positive control** (does the detection string actually fire?): yes.
  `cargo tree -p holon-integration-tests -e normal -i proptest` prints
  `proptest v1.10.0` rooted through `holon` (pbt feature path) — the test's
  `stdout.contains("proptest v")` signal is real, not vacuous.

### Finding W2-1 (LOW-MEDIUM, pre-existing, same class as the W2 target):
tokio `test-util` + opentelemetry `testing` unified into prod graphs

- workspace-hack/Cargo.toml ships, as **normal** `[dependencies]`:
  `tokio = { ..., features = ["full", "test-util", "tracing"] }` and
  `opentelemetry = { ..., features = [..., "testing"] }` (same in the otel_sdk line).
- Confirmed in the prod graph: `cargo tree -p holon-tui -e normal -f "{p} {f}"`
  shows `tokio v1.50.0 ...,test-util,...` and
  `opentelemetry v0.31.0 ...,testing,...`.
- Root cause: hakari unifies workspace members' **dev**-dep features into
  workspace-hack's normal deps by design. Sources:
  - crates/holon-macros/Cargo.toml:21 and crates/holon-macros-test/Cargo.toml:16 —
    `tokio = { workspace = true, features = ["macros", "test-util"] }` in
    `[dev-dependencies]`.
  - opentelemetry `testing` comes from some crate's dev/test usage (not audited to
    the source; grep for `opentelemetry.*testing` when fixing).
- Concrete failure: prod binaries (holon-tui, holon-mcp, adapters) compile tokio
  with the paused-clock test machinery and otel's in-memory test exporters.
  No extra *crates* are dragged in (unlike gpui/proptest), so blast radius is
  code-size/feature hygiene, not dependency-graph pollution. This predates W2 and
  is NOT a regression from it.
- Fix options: (a) audit whether the macros tests actually need `test-util`
  (tokio::test with `start_paused` does; plain `#[tokio::test]` doesn't) and drop
  the feature; (b) if needed, accept and document — traversal-excluding tokio
  would gut hakari's main caching win.

### Finding W2-2 (MEDIUM): regression test can false-pass on cargo failure

- crates/holon-architecture-tests/tests/architecture_rules.rs:65-95.
- The assert is `!linked && !ambiguous` — it only checks for **presence**
  signals (`stdout.contains("proptest v")`, stderr `is ambiguous`). It never
  checks the exit status or requires a recognized **absence** signal.
- Concrete false-pass: rename/typo of `holon-tui` or `holon-mcp` → cargo errors
  `did not match any packages`, stdout empty → test green forever. Same for a
  corrupt Cargo.lock, or `cargo tree` failing to fetch the zed-fork git deps in
  an offline/sandboxed CI — any total failure of the probe reads as "clean".
- Fix: require one of the two documented absence signals when nothing printed:
  `assert!(stderr.contains("nothing to print") || stderr.contains("did not match
  any packages"), "unexpected cargo tree output: ...")` in the pass path.

### Finding W2-3 (LOW-MEDIUM): guard is proptest-only; gpui itself unguarded

- The W2 goal was proptest **and GPUI** out of prod graphs, and the doc comment
  says so, but the test only probes `-i proptest`. If the gpui fork ever drops
  its proptest edge, a hakari regen re-adding gpui to workspace-hack would put
  the whole GPUI framework back into holon-tui/holon-mcp prod builds with the
  test staying green.
- Fix: add a second probe loop `-i gpui` (pass signal is `did not match any
  packages`, since gpui is a git dep absent from those graphs entirely).

### Finding W2-4 (LOW): adapters named in the doc comment but not asserted

- Doc comment (lines 60-62) claims the storage adapters are protected; the loop
  covers only holon-tui + holon-mcp. holon-turso / holon-loro / holon-filesystem
  are clean today (verified above); add them to the `for pkg` list — cost is one
  `cargo tree` each (~2s total measured).

### Cargo.lock drift — clean

- `git show 4b8880080b -- Cargo.lock`: 64 +/- lines total. Content: (1) removal
  of the `holon-markdown` package block, (2) workspace-hack's dep-list churn
  (gpui/collections/http_client/half/slotmap/... out; icu_*/vergen*/yoke/zerovec
  in — these are hakari list edits, not version changes), (3) gpui-mobile pin
  bump fa778004 → f4cb261d (the known iOS insert_text RefCell crash fix —
  separate mobile track co-landed in the same squashed commit; intentional per
  session memory, but note it rode along with W2/W4 rather than landing alone).
- Zero third-party `version =` changes. ed25519-dalek stays 3.0.0-pre.1 /
  pkcs8 0.11.0-rc.11 / iroh 0.96.1 — no ed25519/iroh churn; and the successful
  holon-turso check proves the iroh stack still compiles.

### Note: `markdown` crate in workspace-hack is NOT W4 debris

- workspace-hack still lists `markdown = { version = "1", features = ["serde"] }`.
  `cargo tree -i markdown -e normal` shows the real consumer:
  gpui-component v0.5.1 (path dep at /Users/martin/Workspaces/rust/gpui-component)
  → holon-gpui, as a normal dep. Legitimate. (The out-of-repo local path dep on
  gpui-component is its own reproducibility hazard, but pre-existing and out of
  scope here.)

## W4 — holon-markdown deletion

### Verified solid

- Root Cargo.toml: no holon-markdown in `members` (lines 2-31) or
  `workspace.dependencies` (rg over the file: zero hits).
- Cargo.lock: no `holon-markdown` package (rg exit 1).
- Repo-wide grep (`--hidden`, excluding .git/.jj/devlog/docs) for
  `holon_markdown|holon-markdown`: **zero hits in code, tests, benches, build
  scripts, feature flags, CI (.github/), justfile, .config/**. Remaining
  mentions are only: historical codev/specs/0006+0007 (fine, historical record),
  a conditional-mood comment at crates/holon-core/src/file_format.rs:7
  ("holon-markdown *would* provide", fine), and CONTEXT.md (below).
- No cfg/feature gates reference markdown anywhere in crates/frontends/tools.
- FileFormatAdapter seam survives org-only: it is a trait with runtime
  `extensions()` dispatch (crates/holon-core/src/file_format.rs:42-45), the sole
  impl is OrgFormatAdapter (crates/holon-orgmode/src/file_format.rs:33 returns
  `&["org"]`), and the consumer filters by `self.format.extensions()`
  (crates/holon-filesystem/src/file_sync_controller.rs:1706-1709). **No enum, no
  match arm, no registry entry ever assumed markdown existed** — the design was
  open-world, so deletion leaves no dangling variant. The one hard-coded
  `.with_extension("org")` (file_sync_controller.rs:2059) is a pre-existing
  single-format assumption, consistent with org-only.
- Prod compile proof: the cargo check above covers the filesystem/orgmode stack
  transitively via holon-tui.

### Finding W4-1 (COSMETIC): empty crate dir left on disk

- `crates/holon-markdown/` still exists containing only an untracked `.DS_Store`.
  jj/git don't track dirs, so this survives checkout and shows up in `ls crates`
  — mildly misleading for humans and `ls`-based tooling. Fix: `rm -rf
  crates/holon-markdown` (with .DS_Store hygiene in global gitignore).

### Finding W4-2 (LOW, doc drift): CONTEXT.md still lists the crate as live

- CONTEXT.md:28 — the "Authoring / Format" layer row still names
  `holon-markdown` alongside holon-org-format/holon-orgmode. CONTEXT.md is the
  domain-model source of truth agents load; a future agent may go looking for
  the crate or, worse, re-create it to match the doc. Fix: drop it from the row
  (or annotate "deleted 2026-07; org-only by decision").

## Commands run (for replay)

```
cargo tree -p {holon-tui,holon-mcp,holon-turso,holon-loro,holon-filesystem} \
  -e normal -i {proptest,gpui,wgpu,naga,collections,http_client}
cargo tree -p holon-integration-tests -e normal -i proptest   # positive control
cargo tree -p holon-tui -e normal -f "{p} {f}" | grep -E "tokio|opentelemetry"
cargo hakari verify                                            # exit 0
cargo check -p holon-tui -p holon-mcp -p holon-turso           # exit 0
cargo test -p holon-architecture-tests --test architecture_rules \
  no_proptest_in_prod_graph                                    # ok
git show 4b8880080b -- Cargo.lock                              # drift audit
```
