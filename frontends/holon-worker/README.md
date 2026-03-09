# holon-worker

Holon backend running as a Web Worker on `wasm32-wasip1-threads`.

**Status:** Phase 1 spike complete (2026-04-14). See
`~/.claude/plans/nifty-bouncing-ladybug.md` for the full plan.

## What the Phase 1 spike validated

| Hypothesis | Result |
|---|---|
| **H2** — `napi build` works from holon's repo (not Turso's) | ✅ VALIDATED |
| **H3** — Two wasm targets in one workspace | ✅ Use out-of-workspace crate |
| **H4 step 1** — `tokio::time::sleep` in `#[napi] async fn` | ✅ VALIDATED (69ms RTT) |
| **H4 step 2a** — `Builder::new_multi_thread()` | ❌ FALSIFIED (tokio refuses to compile) |
| **H4 step 2b** — `Builder::new_current_thread()` + `block_on` | ✅ VALIDATED |

### Critical constraint: wasm runs inside a dedicated Web Worker

`tokio`'s `block_on` uses `Atomics.wait` for thread parking. Browsers reject
`Atomics.wait` on the main thread with
`"Atomics.wait cannot be called in this context"`. So the wasm module MUST
load inside a dedicated Web Worker. Main thread talks to it via
`postMessage`. This is a browser security constraint, not a napi/tokio bug.

## Layout

```
Cargo.toml             # [workspace] empty — opts out of holon root workspace
build.rs               # napi_build::setup()
src/lib.rs             # #[napi] pub async fn ping + fn spawn_check
src/turso_browser_shim.rs  # vendored OPFS shim, gated on `feature = "browser"`
package.json           # devDep on @napi-rs/cli + emnapi (pulls static libs)
scripts/build.sh       # reproducible napi build invocation
scripts/serve.mjs      # COOP/COEP dev server with bare-specifier rewriting
web/index.html         # Phase 1 harness page (loads the worker, reports H4 results)
web/worker-entry.mjs   # Dedicated Web Worker that imports the napi glue
```

After `./scripts/build.sh` (or `npm run build`), these files are produced
alongside the crate root:

```
holon_worker.wasm32-wasi.wasm          # 174KB, stripped
holon_worker.wasi-browser.js           # napi-generated ESM glue
wasi-worker-browser.mjs                # emnapi child-thread stub
index.d.ts                             # TypeScript bindings
browser.js                             # re-export entry
```

## Quickstart

```bash
# one-time
rustup target add wasm32-wasip1-threads
cd frontends/holon-worker
npm install

# build the wasm
./scripts/build.sh

# serve the harness with COOP/COEP headers
node scripts/serve.mjs 8088
# open http://127.0.0.1:8088/web/index.html
```

Expected output on the page:

```
worker ready
exports: default, ping, spawnCheck
H4.2 spawnCheck() → current_thread ok: sum=6 (expected 6)
H4.1 ping('phase-1') → "hello phase-1" in ~70ms
```

## Why an out-of-workspace crate

Holon's root workspace contains crates targeting `wasm32-unknown-unknown`
(dioxus-web) and native (everything else). Adding a third target
`wasm32-wasip1-threads` would break `cargo check --workspace` in both
directions. The worker crate therefore declares its own `[workspace]` table
and has its own `Cargo.lock`. Cargo sees it as an independent package.
Holon's root `Cargo.toml` lists it in `exclude` so no workspace-wide build
accidentally descends into it.

## Upgrading Turso

The vendored `src/turso_browser_shim.rs` is a copy of
`bindings/javascript/src/browser.rs` from the Turso repo. Original
upstream commit: `84b440e70eae8f0943e57f0535007e164cb9e294`.

To re-sync after a Turso upgrade:

```bash
diff ~/Workspaces/bigdata/turso/bindings/javascript/src/browser.rs \
     src/turso_browser_shim.rs
```

Port any meaningful changes, update the commit hash at the top of the
shim file, and re-run `./scripts/build.sh`.

## Why we don't depend on `turso_node` directly

`turso_node` is the Rust cdylib that Turso's wasm npm package builds
against. It lives inside Turso's workspace and path-depends on a
sibling `workspace-hack` crate that is not addressable from holon's
workspace. Even if that resolved, `turso_node` drags in the full Node
binding surface (Database, Statement, the napi glue for Node APIs we
don't use). Vendoring the small OPFS subset of `browser.rs` keeps the
compile surface minimal.

## Dev server's bare-specifier rewriting

The napi-generated glue imports `@napi-rs/wasm-runtime`,
`@emnapi/runtime`, etc. as bare module specifiers. Browsers can't
resolve those without a bundler, and importmaps don't propagate into
Web Workers. `scripts/serve.mjs` rewrites bare imports to absolute
`/node_modules/...` URLs on the fly for the spike.

Phase 2 will replace this with a proper Vite build that bundles the
runtime stack into a single ESM file.

## Next steps (Phase 2+)

See `~/.claude/plans/nifty-bouncing-ladybug.md`:

1. Enable the `browser` feature: add `turso_core` as a dep, gate the
   OPFS shim in.
2. Wire the OPFS bridge TS-side (`web/opfs-bridge.ts` — copied from
   Turso's `packages/wasm-common`).
3. Seed + query a persistent OPFS database from the worker; reload and
   re-query.
