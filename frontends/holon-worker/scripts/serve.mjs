// Minimal COOP/COEP-aware static file server for the Phase 1 spike harness.
//
// Why a custom server: `python3 -m http.server` does NOT send the
// Cross-Origin-Opener-Policy / Cross-Origin-Embedder-Policy headers required
// for `crossOriginIsolated === true` and `SharedArrayBuffer`, and we need both
// for emnapi's worker pool on wasm32-wasip1-threads.
//
// Usage: node scripts/serve.mjs [port]      (default port 8088)
//
// Serves the holon-worker root so the wasm, generated glue, vendored
// node_modules, and the web/ harness page are all addressable via plain URLs.
// `/` redirects to `/web/index.html`.

import { createServer } from 'node:http'
import { createReadStream, readFileSync, statSync } from 'node:fs'
import { join, extname, normalize } from 'node:path'

// Bare-specifier → absolute URL map. The napi-rs wasm-runtime stack uses
// node-style package imports; browsers don't do that, and importmaps don't
// propagate into Web Workers. The dev server rewrites `from '@scope/name'`
// on the fly so both the document and the worker see resolvable URLs.
// Phase 2 will replace this with a proper Vite bundle.
const BARE_IMPORTS = {
  '@napi-rs/wasm-runtime':  '/node_modules/@napi-rs/wasm-runtime/runtime.js',
  '@emnapi/runtime':        '/node_modules/@emnapi/runtime/dist/emnapi.mjs',
  '@emnapi/core':           '/node_modules/@emnapi/core/dist/emnapi-core.mjs',
  '@emnapi/wasi-threads':   '/node_modules/@emnapi/wasi-threads/dist/wasi-threads.mjs',
  '@tybys/wasm-util':       '/node_modules/@tybys/wasm-util/lib/mjs/index.mjs',
}

function rewriteBareImports(src) {
  for (const [bare, abs] of Object.entries(BARE_IMPORTS)) {
    const escaped = bare.replace(/[/.\-]/g, c => '\\' + c)
    // Match: `from 'X'`, `from "X"`, `import 'X'`, `import("X")`
    const re = new RegExp(
      `((?:from|import)\\s*\\(?\\s*['"])${escaped}(['"]\\)?)`,
      'g',
    )
    src = src.replace(re, (_m, a, b) => a + abs + b)
  }
  return src
}

const ROOT = new URL('..', import.meta.url).pathname
const PORT = parseInt(process.argv[2] ?? '8088', 10)

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js':   'text/javascript; charset=utf-8',
  '.mjs':  'text/javascript; charset=utf-8',
  '.cjs':  'text/javascript; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.wasm': 'application/wasm',
  '.css':  'text/css; charset=utf-8',
  '.map':  'application/json',
}

const server = createServer((req, res) => {
  // COOP/COEP — required for crossOriginIsolated and SharedArrayBuffer.
  res.setHeader('Cross-Origin-Opener-Policy', 'same-origin')
  res.setHeader('Cross-Origin-Embedder-Policy', 'require-corp')
  res.setHeader('Cross-Origin-Resource-Policy', 'same-origin')
  res.setHeader('Cache-Control', 'no-store')

  let urlPath = decodeURIComponent(new URL(req.url, 'http://localhost').pathname)
  if (urlPath === '/' || urlPath === '') urlPath = '/web/index.html'

  // Defend against path traversal — normalize, then verify the resolved path
  // is still inside ROOT.
  const filePath = normalize(join(ROOT, urlPath))
  if (!filePath.startsWith(ROOT)) {
    res.statusCode = 403
    res.end('forbidden')
    return
  }

  let st
  try {
    st = statSync(filePath)
  } catch {
    res.statusCode = 404
    res.end(`not found: ${urlPath}`)
    return
  }

  if (st.isDirectory()) {
    res.statusCode = 404
    res.end('directory listings disabled')
    return
  }

  const ext = extname(filePath).toLowerCase()
  const mime = MIME[ext] ?? 'application/octet-stream'
  res.setHeader('Content-Type', mime)

  // Rewrite bare specifiers in JS/MJS files on the fly. Do not stream —
  // readFileSync is fine for the spike, all files are tiny.
  if (ext === '.js' || ext === '.mjs' || ext === '.cjs') {
    const body = rewriteBareImports(readFileSync(filePath, 'utf8'))
    res.setHeader('Content-Length', Buffer.byteLength(body))
    res.end(body)
    return
  }

  res.setHeader('Content-Length', st.size)
  createReadStream(filePath).pipe(res)
})

server.listen(PORT, '127.0.0.1', () => {
  console.log(`[holon-worker] serving ${ROOT} on http://127.0.0.1:${PORT}`)
  console.log(`[holon-worker] open http://127.0.0.1:${PORT}/web/index.html`)
})
