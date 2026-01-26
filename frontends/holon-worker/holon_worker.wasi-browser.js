import {
  createOnMessage as __wasmCreateOnMessageForFsProxy,
  getDefaultContext as __emnapiGetDefaultContext,
  instantiateNapiModuleSync as __emnapiInstantiateNapiModuleSync,
  WASI as __WASI,
} from '@napi-rs/wasm-runtime'



const __wasi = new __WASI({
  version: 'preview1',
})

const __wasmUrl = new URL('./holon_worker.wasm32-wasi.wasm', import.meta.url).href
const __emnapiContext = __emnapiGetDefaultContext()


const __sharedMemory = new WebAssembly.Memory({
  initial: 4000,
  maximum: 65536,
  shared: true,
})

const __wasmFile = await fetch(__wasmUrl).then((res) => res.arrayBuffer())

const {
  instance: __napiInstance,
  module: __wasiModule,
  napiModule: __napiModule,
} = __emnapiInstantiateNapiModuleSync(__wasmFile, {
  context: __emnapiContext,
  asyncWorkPoolSize: 4,
  wasi: __wasi,
  onCreateWorker() {
    const worker = new Worker(new URL('./wasi-worker-browser.mjs', import.meta.url), {
      type: 'module',
    })


    return worker
  },
  overwriteImports(importObject) {
    importObject.env = {
      ...importObject.env,
      ...importObject.napi,
      ...importObject.emnapi,
      memory: __sharedMemory,
    }
    return importObject
  },
  beforeInit({ instance }) {
    for (const name of Object.keys(instance.exports)) {
      if (name.startsWith('__napi_register__')) {
        instance.exports[name]()
      }
    }
  },
})
export default __napiModule.exports
export const Opfs = __napiModule.exports.Opfs
export const OpfsFile = __napiModule.exports.OpfsFile
export const completeOpfs = __napiModule.exports.completeOpfs
export const dbExecute = __napiModule.exports.dbExecute
export const dbQuery = __napiModule.exports.dbQuery
export const engineDropSubscription = __napiModule.exports.engineDropSubscription
export const engineExecuteOperation = __napiModule.exports.engineExecuteOperation
export const engineExecuteQuery = __napiModule.exports.engineExecuteQuery
export const engineExecuteSql = __napiModule.exports.engineExecuteSql
export const engineInit = __napiModule.exports.engineInit
export const engineReactiveCheck = __napiModule.exports.engineReactiveCheck
export const engineSetVariant = __napiModule.exports.engineSetVariant
export const engineTick = __napiModule.exports.engineTick
export const engineWatchView = __napiModule.exports.engineWatchView
export const initThreadPool = __napiModule.exports.initThreadPool
export const openDb = __napiModule.exports.openDb
export const ping = __napiModule.exports.ping
export const spawnCheck = __napiModule.exports.spawnCheck
