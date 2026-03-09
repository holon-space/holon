#!/usr/bin/env node
// End-to-end test: initialize → notify → tools/call → verify result
import { request } from 'node:http'

const PORT = parseInt(process.env.PORT ?? '8766', 10)

function post(path, body, headers = {}) {
  return new Promise((resolve, reject) => {
    const data = JSON.stringify(body)
    const req = request({
      hostname: '127.0.0.1',
      port: PORT,
      path,
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Accept': 'application/json, text/event-stream',
        'Content-Length': Buffer.byteLength(data),
        ...headers,
      },
    }, res => {
      let buf = ''
      const sessionId = res.headers['mcp-session-id']
      res.on('data', chunk => { buf += chunk })
      res.on('end', () => resolve({ status: res.statusCode, body: buf, sessionId }))
    })
    req.on('error', reject)
    req.write(data)
    req.end()
  })
}

function parseSSE(body) {
  for (const line of body.split('\n')) {
    if (line.startsWith('data: ')) {
      try { return JSON.parse(line.slice(6)) } catch { /* ignore */ }
    }
  }
  return null
}

async function run() {
  console.log(`[test] connecting to http://127.0.0.1:${PORT}/mcp`)

  // 1. Initialize
  const initRes = await post('/mcp', {
    jsonrpc: '2.0', id: 1, method: 'initialize',
    params: {
      protocolVersion: '2024-11-05',
      capabilities: {},
      clientInfo: { name: 'test-relay', version: '1' },
    },
  })
  const sessionId = initRes.sessionId
  const initData = parseSSE(initRes.body)
  console.log('[test] initialize:', initData?.result?.serverInfo ?? initData)
  if (!sessionId) { console.error('[test] ERROR: no mcp-session-id in response'); process.exit(1) }
  console.log('[test] session:', sessionId)

  // 2. Notify initialized
  const notifyRes = await post('/mcp', {
    jsonrpc: '2.0', method: 'notifications/initialized',
  }, { 'mcp-session-id': sessionId })
  console.log('[test] notifications/initialized HTTP status:', notifyRes.status)

  // 3. List tools
  const listRes = await post('/mcp', {
    jsonrpc: '2.0', id: 2, method: 'tools/list', params: {},
  }, { 'mcp-session-id': sessionId })
  const listData = parseSSE(listRes.body)
  const tools = listData?.result?.tools ?? []
  console.log(`[test] tools/list: ${tools.length} tools (${tools.slice(0,3).map(t=>t.name).join(', ')}...)`)

  // 4. Call execute_query
  const callRes = await post('/mcp', {
    jsonrpc: '2.0', id: 3, method: 'tools/call',
    params: { name: 'execute_query', arguments: { query: 'SELECT id FROM block LIMIT 5', language: 'holon_sql' } },
  }, { 'mcp-session-id': sessionId })
  const callData = parseSSE(callRes.body)
  console.log('[test] execute_query result:', JSON.stringify(callData?.result ?? callData?.error))

  if (callData?.result && !callData.result.isError) {
    console.log('[test] PASS ✓')
  } else {
    console.log('[test] FAIL ✗')
    process.exit(1)
  }
}

run().catch(err => { console.error('[test] fatal:', err); process.exit(1) })
