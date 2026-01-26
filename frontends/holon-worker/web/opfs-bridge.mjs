// OPFS sync-handle bridge for the holon-worker wasm module.
//
// Adapted from Turso's bindings/javascript/packages/wasm-common/index.ts
// (upstream commit 84b440e70eae8f0943e57f0535007e164cb9e294).
//
// ## Architecture divergence from upstream
//
// Turso's wasm package runs the wasm module on the **main thread** with a
// **dedicated worker** holding the OPFS sync access handles. Turso can get
// away with this because its Rust IO path uses `Completion::wait`, which
// yields to JS rather than blocking — so it never calls `Atomics.wait` on
// the main thread.
//
// Holon is different. `BackendEngine` is driven by a `tokio` current-thread
// runtime whose `block_on` parks via `Atomics.wait`. That is forbidden on
// the main thread in browsers. So we run the entire wasm module (BOTH the
// napi "main" instance AND its child-thread workers) inside a single
// dedicated Web Worker — the one that imported this file. The OPFS sync
// handles live in the same worker, and the Rust shim's `is_web_worker()`
// always returns true, so the synchronous OPFS path is the only one in
// use.
//
// The *_async imports are still provided but panic if called — they
// should never be called in our topology.

function getUint8ArrayFromMemory(memory, ptr, len) {
  ptr = ptr >>> 0
  return new Uint8Array(memory.buffer).subarray(ptr, ptr + len)
}

function getStringFromMemory(memory, ptr, len) {
  const shared = getUint8ArrayFromMemory(memory, ptr, len)
  const copy = new Uint8Array(shared.length)
  copy.set(shared)
  return new TextDecoder('utf-8').decode(copy)
}

export class OpfsDirectory {
  constructor() {
    this.fileByPath = new Map()
    this.fileByHandle = new Map()
    this.fileHandleNo = 0
  }

  async registerFile(path) {
    if (this.fileByPath.has(path)) return
    const root = await navigator.storage.getDirectory()
    const handle = await root.getFileHandle(path, { create: true })
    const sync = await handle.createSyncAccessHandle()
    this.fileHandleNo += 1
    this.fileByPath.set(path, { handle: this.fileHandleNo, sync })
    this.fileByHandle.set(this.fileHandleNo, sync)
  }

  async unregisterFile(path) {
    const file = this.fileByPath.get(path)
    if (file == null) return
    this.fileByPath.delete(path)
    this.fileByHandle.delete(file.handle)
    file.sync.close()
  }

  lookupFileHandle(path) {
    const file = this.fileByPath.get(path)
    return file == null ? null : file.handle
  }

  read(handle, buffer, offset) {
    return this.fileByHandle.get(handle).read(buffer, { at: Number(offset) })
  }

  write(handle, buffer, offset) {
    return this.fileByHandle.get(handle).write(buffer, { at: Number(offset) })
  }

  sync(handle) {
    this.fileByHandle.get(handle).flush()
    return 0
  }

  truncate(handle, size) {
    this.fileByHandle.get(handle).truncate(size)
    return 0
  }

  size(handle) {
    return this.fileByHandle.get(handle).getSize()
  }
}

// Factory: build the import object that the Rust `turso_browser_shim` expects.
// `getMemory` is a thunk so we can set the memory after the wasm is
// instantiated — the Rust shim copies strings/buffers out of the shared
// memory, so we need a live reference.
export function opfsWorkerImports(opfs, getMemory) {
  const mem = () => getMemory()
  const panicWorker = (name) => {
    throw new Error(`method ${name} must only be invoked via the async path, but the worker topology is all-sync`)
  }
  return {
    is_web_worker: () => true,

    lookup_file: (ptr, len) => {
      try {
        const handle = opfs.lookupFileHandle(getStringFromMemory(mem(), ptr, len))
        return handle == null ? -404 : handle
      } catch (e) {
        console.error('lookup_file', e)
        return -1
      }
    },

    read: (handle, ptr, len, offset) => {
      try {
        return opfs.read(handle, getUint8ArrayFromMemory(mem(), ptr, len), offset)
      } catch (e) {
        console.error('read', handle, len, offset, e)
        return -1
      }
    },

    write: (handle, ptr, len, offset) => {
      try {
        return opfs.write(handle, getUint8ArrayFromMemory(mem(), ptr, len), offset)
      } catch (e) {
        console.error('write', handle, len, offset, e)
        return -1
      }
    },

    sync: (handle) => {
      try {
        return opfs.sync(handle)
      } catch (e) {
        console.error('sync', handle, e)
        return -1
      }
    },

    truncate: (handle, len) => {
      try {
        return opfs.truncate(handle, len)
      } catch (e) {
        console.error('truncate', handle, len, e)
        return -1
      }
    },

    size: (handle) => {
      try {
        return opfs.size(handle)
      } catch (e) {
        console.error('size', handle, e)
        return -1
      }
    },

    // Async variants — never called in our topology because `is_web_worker`
    // returns true above. Panicking here gives us a clear failure if that
    // assumption ever breaks.
    read_async: () => panicWorker('read_async'),
    write_async: () => panicWorker('write_async'),
    sync_async: () => panicWorker('sync_async'),
    truncate_async: () => panicWorker('truncate_async'),
  }
}
