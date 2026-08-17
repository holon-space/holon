---
id: 2026-07-12-napi-wasm-runtime-tybys-wasm-util
date: 2026-07-12
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  @napi-rs/wasm-runtime (+@tybys/wasm-util) missing from holon-worker
  package.json — worker glue import fails opaquely (`worker spawn: worker
  error` on a fresh workspace after npm install)
source_line: 984
---

## Bug

@napi-rs/wasm-runtime (+@tybys/wasm-util) missing from holon-worker
package.json — worker glue import fails opaquely (`worker spawn: worker
error` on a fresh workspace after npm install)

## Missing piece

no install-from-lockfile CI for worker glue

## Remedy

FIXED (B5): `@napi-rs/wasm-runtime` pinned to 1.1.3 in holon-worker
package.json `dependencies` (pulls @tybys/wasm-util 0.10.3 transitively; not
imported directly so not declared)
