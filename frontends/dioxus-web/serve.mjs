#!/usr/bin/env node
// Combined dev server for holon-dioxus-web.
process.on('uncaughtException', err => console.error('[serve] uncaughtException:', err))
process.on('unhandledRejection', err => console.error('[serve] unhandledRejection:', err))
//
// Serves:
//   /               → Trunk-built Dioxus app  (frontends/dioxus-web/dist/)
//   /web/*          → holon-worker web harness (frontends/holon-worker/web/)
//   /node_modules/* → worker npm deps          (frontends/holon-worker/node_modules/)
//   /holon_worker.wasm32-wasi.wasm
//                   → worker wasm, served directly from cargo target/
//                     (release-official preferred, falls back to debug). Keeps the
//                     631 MB dev binary out of the VCS-visible worker package root.
//   /mcp, /mcp/*    → HTTP proxy to native MCP relay at RELAY_PORT (default 3002)
//   /mcp-hub        → WebSocket hub bridging native relay ↔ browser
//
// All responses carry COOP/COEP headers (required for wasm-wasip1-threads).
//
// Usage:
//   node serve.mjs            # serve only (dist/ must already exist)
//   node serve.mjs --build    # run `trunk build` first, then serve
//   node serve.mjs --watch    # run `trunk watch` in background + serve
//
// Default port: 8765. Override: PORT=9000 node serve.mjs
// Relay port:   3002. Override: RELAY_PORT=4000 node serve.mjs

import { createServer, request as httpRequest } from 'node:http'
import { createReadStream, readFileSync, statSync, existsSync } from 'node:fs'
import { join, extname, normalize } from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync, spawn } from 'node:child_process'
import { WebSocketServer } from 'ws'

const HERE = fileURLToPath(new URL('.', import.meta.url))
const WORKER_ROOT = join(HERE, '../holon-worker')
const WORKER_TARGET = join(WORKER_ROOT, 'target/wasm32-wasip1-threads')
const WORKER_WASM_URL = '/holon_worker.wasm32-wasi.wasm'
const DIST = join(HERE, 'dist')

// napi build always lands at target/wasm32-wasip1-threads/{profile}/holon_worker.wasm
// and the copyArtifact step into the worker root is unreliable on dev profiles
// (looks for `dev/` instead of `debug/`). Serve directly from target/ so the
// 631 MB binary never touches the VCS-visible package root.
//
// If multiple profiles exist, prefer the most recently built one — that's
// almost always what the developer just touched. Override with the
// HOLON_WORKER_WASM env var if you need a specific build.
function resolveWorkerWasm() {
  if (process.env.HOLON_WORKER_WASM && existsSync(process.env.HOLON_WORKER_WASM)) {
    return process.env.HOLON_WORKER_WASM
  }
  const candidates = [
    join(WORKER_TARGET, 'debug/holon_worker.wasm'),
    join(WORKER_TARGET, 'release/holon_worker.wasm'),
    join(WORKER_TARGET, 'release-official/holon_worker.wasm'),
  ].filter(existsSync)
  if (candidates.length === 0) return null
  candidates.sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs)
  return candidates[0]
}
const PORT = parseInt(process.env.PORT ?? '8765', 10)
const RELAY_PORT = parseInt(process.env.RELAY_PORT ?? '3002', 10)

// ── Bare-specifier rewriting (same as holon-worker/scripts/serve.mjs) ────────
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
    const re = new RegExp(
      `((?:from|import)\\s*\\(?\\s*['"])${escaped}(['"]\\)?)`,
      'g',
    )
    src = src.replace(re, (_m, a, b) => a + abs + b)
  }
  return src
}

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

// ── Build step ────────────────────────────────────────────────────────────────
const args = process.argv.slice(2)
if (args.includes('--build')) {
  console.log('[serve] running trunk build…')
  const r = spawnSync('trunk', ['build'], {
    cwd: HERE,
    stdio: 'inherit',
    env: { ...process.env },
  })
  if (r.status !== 0) {
    console.error('[serve] trunk build failed')
    process.exit(1)
  }
}

if (args.includes('--watch')) {
  console.log('[serve] starting trunk watch in background…')
  const w = spawn('trunk', ['watch'], {
    cwd: HERE,
    stdio: 'inherit',
    env: { ...process.env },
    detached: false,
  })
  w.on('exit', code => console.log(`[trunk watch] exited (${code})`))
}

if (!existsSync(DIST)) {
  console.error(`[serve] dist/ not found at ${DIST}`)
  console.error('[serve] run with --build first: node serve.mjs --build')
  process.exit(1)
}

// ── WebSocket hub ─────────────────────────────────────────────────────────────
// Bridges the native MCP relay (role=native) and the Dioxus browser page
// (role=browser). Tool call requests flow native→browser; responses flow
// browser→native. Requests are multiplexed by id so multiple concurrent MCP
// sessions share a single WebSocket connection.
const wss = new WebSocketServer({ noServer: true })
const hub = { native: null, browser: null }

wss.on('connection', (ws, req) => {
  const url = new URL(req.url, 'http://localhost')
  const role = url.searchParams.get('role')
  if (role !== 'native' && role !== 'browser') {
    ws.close(1008, 'role must be native or browser')
    return
  }

  if (hub[role]) {
    console.log(`[mcp-hub] replacing existing ${role} connection`)
    hub[role].close(1001, 'replaced by new connection')
  }
  hub[role] = ws
  console.log(`[mcp-hub] ${role} connected`)

  ws.on('message', (data, isBinary) => {
    const peer = role === 'native' ? hub.browser : hub.native
    if (peer?.readyState === 1 /* OPEN */) {
      peer.send(data, { binary: isBinary })
    } else {
      console.warn(`[mcp-hub] ${role} sent message but peer not connected`)
    }
  })

  ws.on('close', () => {
    hub[role] = null
    console.log(`[mcp-hub] ${role} disconnected`)
  })

  ws.on('error', err => console.error(`[mcp-hub] ${role} error:`, err.message))
})

// ── Request handler ───────────────────────────────────────────────────────────
function resolveFile(urlPath) {
  // The worker wasm is served directly from cargo target/ to keep the giant
  // binary out of the VCS-visible package root. resolveWorkerWasm() prefers
  // release-official > release > debug.
  if (urlPath === WORKER_WASM_URL) {
    return resolveWorkerWasm()
  }

  // /web/* and /node_modules/* always come from the worker package.
  if (urlPath.startsWith('/web/') || urlPath === '/web') {
    return normalize(join(WORKER_ROOT, urlPath))
  }
  if (urlPath.startsWith('/node_modules/')) {
    return normalize(join(WORKER_ROOT, urlPath))
  }

  // For everything else: check Trunk dist first, then worker root.
  // This correctly serves both the Dioxus wasm (in dist/) and the
  // holon_worker wasm (in the worker root) without path-pattern guessing.
  const distPath = normalize(join(DIST, urlPath))
  if (distPath.startsWith(DIST) && existsSync(distPath) && !statSync(distPath).isDirectory()) {
    return distPath
  }
  const workerPath = normalize(join(WORKER_ROOT, urlPath))
  if (workerPath.startsWith(WORKER_ROOT) && existsSync(workerPath) && !statSync(workerPath).isDirectory()) {
    return workerPath
  }

  // SPA fallback for Dioxus client-side routes.
  return join(DIST, 'index.html')
}

const server = createServer((req, res) => {
  res.setHeader('Cross-Origin-Opener-Policy', 'same-origin')
  res.setHeader('Cross-Origin-Embedder-Policy', 'require-corp')
  res.setHeader('Cross-Origin-Resource-Policy', 'same-origin')
  res.setHeader('Cache-Control', 'no-store')

  const urlPath = decodeURIComponent(new URL(req.url, 'http://localhost').pathname)

  // ── /mcp proxy → native relay ───────────────────────────────────────────────
  // Forwards all MCP HTTP traffic to the native relay at RELAY_PORT so Claude
  // Code only needs one URL: http://localhost:PORT/mcp
  if (urlPath === '/mcp' || urlPath.startsWith('/mcp/')) {
    const options = {
      hostname: '127.0.0.1',
      port: RELAY_PORT,
      path: req.url,
      method: req.method,
      headers: req.headers,
    }
    const proxy = httpRequest(options, upstream => {
      if (!res.headersSent) res.writeHead(upstream.statusCode, upstream.headers)
      upstream.on('error', () => res.destroy())
      res.on('close', () => upstream.destroy())
      upstream.pipe(res, { end: true })
    })
    proxy.on('error', () => {
      if (!res.headersSent) { res.writeHead(502); res.end('MCP relay unavailable') }
    })
    req.on('error', () => proxy.destroy())
    req.pipe(proxy)
    return
  }

  const filePath = resolveFile(urlPath === '/' ? '/index.html' : urlPath)

  if (!filePath) {
    if (urlPath === WORKER_WASM_URL) {
      res.statusCode = 404
      res.end(
        `worker wasm not built — run:\n` +
        `  cd frontends/holon-worker && \\\n` +
        `  EMNAPI_LINK_DIR="$(pwd)/node_modules/emnapi/lib/wasm32-wasi-threads" \\\n` +
        `  ./node_modules/.bin/napi build --features browser --profile dev --platform \\\n` +
        `  --target wasm32-wasip1-threads --no-js --manifest-path ./Cargo.toml --output-dir .\n` +
        `(copyArtifact will fail; the build output at target/.../debug/holon_worker.wasm is what serve.mjs uses)\n`
      )
      return
    }
    res.statusCode = 403
    res.end('forbidden')
    return
  }

  let st
  try { st = statSync(filePath) } catch {
    res.statusCode = 404
    res.end(`not found: ${urlPath}`)
    return
  }

  const ext = extname(filePath).toLowerCase()
  const mime = MIME[ext] ?? 'application/octet-stream'
  res.setHeader('Content-Type', mime)

  if (ext === '.js' || ext === '.mjs' || ext === '.cjs') {
    const body = rewriteBareImports(readFileSync(filePath, 'utf8'))
    res.setHeader('Content-Length', Buffer.byteLength(body))
    res.end(body)
    return
  }

  res.setHeader('Content-Length', st.size)
  createReadStream(filePath).pipe(res)
})

// ── WebSocket upgrade routing ─────────────────────────────────────────────────
// Only /mcp-hub upgrades reach the hub; all others are destroyed.
server.on('upgrade', (req, socket, head) => {
  const url = new URL(req.url, 'http://localhost')
  if (url.pathname !== '/mcp-hub') {
    socket.destroy()
    return
  }
  wss.handleUpgrade(req, socket, head, ws => wss.emit('connection', ws, req))
})

server.listen(PORT, '127.0.0.1', () => {
  console.log(`[holon-dioxus-web] http://127.0.0.1:${PORT}`)
  console.log(`[mcp-hub]          ws://127.0.0.1:${PORT}/mcp-hub`)
  console.log(`[mcp-proxy]        http://127.0.0.1:${PORT}/mcp → :${RELAY_PORT}`)
})
