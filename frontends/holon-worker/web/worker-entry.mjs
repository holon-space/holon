// Dedicated Web Worker that owns the holon-worker wasm + OPFS sync handles.
//
// Architecture: we bypass the napi-generated `holon_worker.wasi-browser.js`
// glue and instantiate the wasm manually here. The only reason is that the
// auto-generated glue's `overwriteImports` callback is fixed and does not
// give us a hook to inject the OPFS bridge. By inlining the instantiation
// we control the import object completely.
//
// Phase 1 finding (H4): tokio current-thread `block_on` uses `Atomics.wait`
// which is forbidden on the main thread — so the entire wasm module runs
// inside this worker. OPFS sync access handles live in the same worker and
// the Rust shim's `is_web_worker()` always returns true.

import {
  instantiateNapiModuleSync as __emnapiInstantiateNapiModuleSync,
  getDefaultContext as __emnapiGetDefaultContext,
  WASI as __WASI,
} from '@napi-rs/wasm-runtime'

import { OpfsDirectory, opfsWorkerImports } from '/web/opfs-bridge.mjs'

// Surface fatal worker errors to the parent. The top-level `await` below means
// any instantiation failure becomes an unhandled rejection whose `error` event
// is opaque (`{isTrusted:true}`) cross-context; forward the real message so the
// page can show it instead of just a ready-timeout.
self.addEventListener('unhandledrejection', (ev) => {
  self.postMessage({ kind: 'fatal', error: String((ev.reason && ev.reason.stack) || ev.reason) })
})
self.addEventListener('error', (ev) => {
  self.postMessage({ kind: 'fatal', error: String((ev.error && ev.error.stack) || ev.message || ev) })
})

// ── Wasm instantiation ────────────────────────────────────────────────────

const __wasi = new __WASI({
  version: 'preview1',
  print: (...args) => console.log('[wasm]', ...args),
  printErr: (...args) => console.error('[wasm]', ...args),
})
const __emnapiContext = __emnapiGetDefaultContext()
const __sharedMemory = new WebAssembly.Memory({
  initial: 4000,
  maximum: 65536,
  shared: true,
})

const opfs = new OpfsDirectory()
const opfsImports = opfsWorkerImports(opfs, () => __sharedMemory)

const __wasmUrl = new URL('/holon_worker.wasm32-wasi.wasm', self.location.origin).href
const __wasmFile = await fetch(__wasmUrl).then(r => r.arrayBuffer())

const { napiModule: __napiModule } = __emnapiInstantiateNapiModuleSync(__wasmFile, {
  context: __emnapiContext,
  asyncWorkPoolSize: 4,
  wasi: __wasi,
  onCreateWorker() {
    // emnapi spawns sub-workers for its async work pool. These child workers
    // re-instantiate the wasm with `childThread: true`. We use our own
    // wrapper (web/wasi-worker-with-opfs-stubs.mjs) instead of the napi-
    // generated `wasi-worker-browser.mjs` because the generated file is
    // clobbered on every build AND lacks the OPFS import stubs that the
    // turso_browser_shim requires for child-thread linking.
    const w = new Worker(new URL('/web/wasi-worker-with-opfs-stubs.mjs', self.location.origin), {
      type: 'module',
    })
    w.addEventListener('message', (ev) => {
      if (ev.data && ev.data.__holon_fatal) {
        self.postMessage({ kind: 'fatal', error: 'child-thread: ' + ev.data.__holon_fatal })
      }
    })
    w.addEventListener('error', (ev) => {
      self.postMessage({ kind: 'fatal', error: 'child-thread onerror: ' + (ev.message || `${ev.filename}:${ev.lineno}`) })
    })
    return w
  },
  overwriteImports(importObject) {
    importObject.env = {
      ...importObject.env,
      ...importObject.napi,
      ...importObject.emnapi,
      ...opfsImports,
      memory: __sharedMemory,
    }
    // wasm-bindgen import stubs.
    //
    // loro-internal (a transitive dep of holon) has a buggy cfg predicate
    // that pulls `wasm-bindgen` for ALL wasm targets — including
    // wasm32-wasip1-threads where we have no JS-side wasm-bindgen-cli
    // shim. Only 3 import symbols are referenced and none of them are
    // reached during the code paths the worker actually executes (Loro
    // is wired into LoroDocumentStore which we sidestep with the
    // `:memory:` BackendEngine init for now). Trivial stubs are enough
    // to make WebAssembly.Instance() succeed at link time.
    //
    // If anything ever calls these we'll find out via the `console.warn`
    // log lines below.
    // wasm-bindgen stubs. Release builds strip most references; dev
    // builds keep many more (notably __wbindgen_throw and friends). Use
    // a Proxy to hand back a lazy stub for any name we haven't already
    // defined — any unexpected call logs once and returns 0/undefined.
    const makeLazyStubModule = (modName) =>
      new Proxy({}, {
        get(_target, name) {
          if (typeof name !== 'string') return undefined
          return (...args) => {
            console.warn(`[holon-worker] ${modName}.${name} called — returning 0 stub`, args)
            return 0
          }
        },
        has() { return true },
      })

    importObject.__wbindgen_placeholder__ = makeLazyStubModule('__wbindgen_placeholder__')
    importObject.__wbindgen_externref_xform__ = makeLazyStubModule('__wbindgen_externref_xform__')
    return importObject
  },
  beforeInit({ instance }) {
    // Initialize the main thread's WASI thread-pointer / pthread state BEFORE
    // any napi registration touches thread-local destructors. napi builds this
    // crate as a wasm library, so crt1-reactor's `_initialize` (which would run
    // `__wasi_init_tp()`) is never exported/called (rust-lang/rust#146843).
    // Without this, the main thread's pthread key subsystem is uninitialized
    // and the first thread-local-destructor registration deadlocks in
    // `pthread_key_create`. See holon_init_main_thread() in src/lib.rs.
    instance.exports.holon_init_main_thread()
    for (const name of Object.keys(instance.exports)) {
      if (name.startsWith('__napi_register__')) {
        instance.exports[name]()
      }
    }
  },
})

const mod = __napiModule.exports

// ── RPC loop ──────────────────────────────────────────────────────────────

self.addEventListener('message', async (e) => {
  const { id, kind, args = [] } = e.data
  try {
    let value
    switch (kind) {
      case 'exports':
        value = Object.keys(mod)
        break
      case 'ping':
        value = await mod.ping(args[0])
        break
      case 'spawnCheck':
        value = mod.spawnCheck()
        break
      case 'registerFile':
        await opfs.registerFile(args[0])
        value = null
        break
      case 'openDb':
        value = mod.openDb(args[0]) ?? null
        break
      case 'dbExecute':
        value = Number(mod.dbExecute(args[0]))
        break
      case 'dbQuery':
        value = mod.dbQuery(args[0])
        break
      case 'engineInit':
        value = mod.engineInit(args[0]) ?? null
        break
      case 'engineResetStorage': {
        // B2: clear local data. Tear the engine down first (Rust releases the
        // Turso OPFS file handles), THEN close the sync-access handles and
        // delete the files. Deleting while sync handles are open throws
        // NoModificationAllowedError, so ordering is load-bearing. args = [dbPath].
        const dbPath = args[0]
        mod.engineResetStorage()
        for (const f of [dbPath, `${dbPath}-wal`, `${dbPath}-shm`]) {
          await opfs.unregisterFile(f)
        }
        const root = await navigator.storage.getDirectory()
        for (const f of [dbPath, `${dbPath}-wal`, `${dbPath}-shm`]) {
          try {
            await root.removeEntry(f)
          } catch (e) {
            // NotFoundError is fine (file may not exist); anything else — e.g.
            // NoModificationAllowedError — means a handle is still open, which
            // is a real bug we must surface, not swallow.
            if (e && e.name !== 'NotFoundError') throw e
          }
        }
        value = null
        break
      }
      case 'engineExecuteSql':
        value = Number(mod.engineExecuteSql(args[0]))
        break
      case 'engineExecuteQuery':
        // Returns a JSON string from Rust; parse before sending back.
        value = JSON.parse(mod.engineExecuteQuery(args[0]))
        break
      case 'engineReactiveCheck':
        value = mod.engineReactiveCheck()
        break
      case 'engineTick':
        // Drive the current-thread runtime for `budget_ms` so spawned
        // tasks (watch_view drains) make progress. JS side should call
        // this from a setInterval / rAF loop.
        mod.engineTick(args[0] ?? 10)
        value = null
        break
      case 'engineWatchView': {
        // Handle is allocated on the Rust side BEFORE the drain task is
        // spawned (see subscriptions::allocate), so `engineWatchView`
        // returns a live handle by the time the callback can ever fire.
        const holder = { id: 0 }
        holder.id = mod.engineWatchView(args[0], (cbArg) => {
          // napi-rs's `ThreadsafeFunction<String>` with
          // `build_callback(|ctx| Ok((ctx.value,)))` declares a 1-tuple
          // of args. On emnapi/wasi-threads the tuple is delivered to
          // JS as a single-element array `[payload]`, NOT as a
          // positional argument. Unwrap before routing.
          let payload = cbArg
          if (Array.isArray(payload) && payload.length === 1) {
            payload = payload[0]
          }
          let json
          if (typeof payload === 'string') {
            json = payload
          } else if (payload && typeof payload === 'object') {
            // Structured JS object: serialize to transport as JSON.
            json = JSON.stringify(payload)
          } else {
            json = String(payload)
          }
          self.postMessage({ kind: 'snapshot', handle: holder.id, snapshotJson: json })
        })
        value = holder.id
        break
      }
      case 'engineDropSubscription':
        mod.engineDropSubscription(args[0])
        value = null
        break
      case 'engineExecuteOperation':
        // args: [entity, op, paramsJson]
        value = JSON.parse(mod.engineExecuteOperation(args[0], args[1], args[2]))
        break
      case 'engineDispatchIntents':
        // args: [intentsJson] — JSON array of {entity, op, params}.
        // Fire-and-forget chain; it advances on subsequent engineTick calls.
        mod.engineDispatchIntents(args[0])
        value = null
        break
      case 'engineSetViewport':
        // args: [widthPx, heightPx, scaleFactor]
        mod.engineSetViewport(args[0], args[1], args[2])
        value = null
        break
      case 'engineSetFocus':
        // args: [blockIdOrNull, caretOffsetOrNull]
        mod.engineSetFocus(args[0] ?? null, args[1] ?? null)
        value = null
        break
      case 'engineSetVariant':
        // args: [blockId, variant]
        mod.engineSetVariant(args[0], args[1])
        value = null
        break
      case 'engineSnapshotView':
        // args: [blockId]
        // Returns JSON string (ViewModel snapshot).
        value = JSON.parse(mod.engineSnapshotView(args[0]))
        break
      case 'engineMcpTool': {
        // args: [toolName, argsJsonString]
        // argsJsonString is the JSON-serialised arguments map.
        // Returns JSON string — parse so the main thread receives a plain object.
        const rawResult = mod.engineMcpTool(args[0], args[1])
        value = JSON.parse(rawResult)
        break
      }
      default:
        throw new Error('unknown kind: ' + kind)
    }
    self.postMessage({ id, ok: true, value })
  } catch (err) {
    self.postMessage({ id, ok: false, error: String(err?.stack || err) })
  }
})

self.postMessage({ kind: 'ready' })
