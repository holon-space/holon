// Stable child-thread worker for holon-worker's emnapi async pool.
//
// We don't reference the auto-generated `wasi-worker-browser.mjs` directly
// because `napi build` regenerates it on every build and clobbers any edits.
// Instead this file owns the child-thread entry permanently and is referenced
// from `web/worker-entry.mjs::onCreateWorker`.
//
// Why this file is necessary: emnapi's async work pool re-instantiates the
// holon-worker wasm module inside each sub-worker (childThread: true,
// shared memory). The wasm imports include the Rust `turso_browser_shim`
// OPFS imports — `lookup_file`, `read`, `write`, `sync`, `truncate`, `size`,
// `is_web_worker`, plus the `_async` variants. If any of those is missing
// from the import object, WebAssembly.Instance() fails at link time with
//
//     LinkError: function import requires a callable
//
// which silently kills the napi async runtime. Symptom: any `#[napi] async
// fn` hangs forever.
//
// Sub-workers don't actually own OPFS sync access handles — those live in
// the main worker (web/worker-entry.mjs). The stubs here therefore panic
// loudly if anything ever tries to invoke them. `is_web_worker` returns
// false so the Rust shim's dispatch never picks the sync path on a child.

import { instantiateNapiModuleSync, MessageHandler, WASI } from '@napi-rs/wasm-runtime'

const panicChild = (name) => () => {
  throw new Error(
    `[holon-worker child thread] unexpectedly called OPFS import '${name}'. ` +
    `OPFS may only be touched from the main worker thread.`
  )
}

const CHILD_OPFS_STUBS = {
  is_web_worker:  () => false,
  lookup_file:    panicChild('lookup_file'),
  read:           panicChild('read'),
  write:          panicChild('write'),
  sync:           panicChild('sync'),
  truncate:       panicChild('truncate'),
  size:           panicChild('size'),
  read_async:     panicChild('read_async'),
  write_async:    panicChild('write_async'),
  sync_async:     panicChild('sync_async'),
  truncate_async: panicChild('truncate_async'),
}

const handler = new MessageHandler({
  onLoad({ wasmModule, wasmMemory }) {
    const wasi = new WASI({
      print: (...args) => console.log('[wasm child]', ...args),
      printErr: (...args) => console.error('[wasm child]', ...args),
    })
    return instantiateNapiModuleSync(wasmModule, {
      childThread: true,
      wasi,
      overwriteImports(importObject) {
        importObject.env = {
          ...importObject.env,
          ...importObject.napi,
          ...importObject.emnapi,
          ...CHILD_OPFS_STUBS,
          memory: wasmMemory,
        }
        // See worker-entry.mjs for why these stubs exist (loro-internal
        // pulls wasm-bindgen on all wasm targets via a buggy cfg).
        importObject.__wbindgen_placeholder__ = {
          __wbindgen_describe: () => {},
        }
        importObject.__wbindgen_externref_xform__ = {
          __wbindgen_externref_table_grow: () => 0,
          __wbindgen_externref_table_set_null: () => {},
        }
      },
    })
  },
})

globalThis.onmessage = (e) => handler.handle(e)
