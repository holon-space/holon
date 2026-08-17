---
id: 2026-07-12-worker-wasm-deadlocks-napi-registration-wasip1
date: 2026-07-12
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  worker wasm deadlocks in napi registration: wasip1-threads cdylib never
  calls __wasi_init_tp (rustc #146843), first TLS-dtor registration blocks
  forever
source_line: 983
---

## Bug

worker wasm deadlocks in napi registration: wasip1-threads cdylib never
calls __wasi_init_tp (rustc #146843), first TLS-dtor registration blocks
forever

## Missing piece

browser worker boot path untestable headless; no boot smoke test

## Remedy

FIXED (exported holon_init_main_thread→__wasi_init_tp before napi init;
0f6dfe44)
