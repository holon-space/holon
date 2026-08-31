// Contract for the WASI-stderr sink. The worker-smoke gate asserts zero
// ERROR-level console lines, so every case here is the difference between a
// gate that can go green and one that can still catch a dying worker.
//
// Nothing here calls a flush by hand: the browser has no such caller, so a
// harness that flushed would attest to a path production never takes. Records
// are fed the way the wasm feeds them — as chunks, sometimes a tick apart — and
// the only flush is the sink's own stale timer.
import { test } from 'node:test'
import assert from 'node:assert/strict'

import { wasmStderrSink, STALE_PARTIAL_MS } from '../web/wasm-log.mjs'

const TS = '2026-08-31T12:10:18.810000Z'

const tick = () => new Promise((r) => setTimeout(r, 0))
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

/// Feeds `chunks` through a sink with every console method captured, and
/// returns `[method, line]` pairs in emission order. `between` runs between
/// chunks — pass `tick` to put an event-loop turn between them, which is where
/// a partial line has to survive.
async function route(chunks, { between = null, settle = 0 } = {}) {
  const seen = []
  const saved = {}
  for (const k of ['error', 'warn', 'info', 'log', 'debug']) {
    saved[k] = console[k]
    console[k] = (_prefix, ...rest) => seen.push([k, rest.join(' ')])
  }
  try {
    const sink = wasmStderrSink('[wasm]')
    for (const c of chunks) {
      sink(c)
      if (between) await between()
    }
    if (settle) await sleep(settle)
  } finally {
    Object.assign(console, saved)
  }
  return seen
}

const methods = async (chunks, opts) => (await route(chunks, opts)).map(([m]) => m)

test('an INFO record is not an error', async () => {
  assert.deepEqual(await methods([`${TS}  INFO holon::api: booted\n`]), ['info'])
})

test('a WARN record is warn, not error', async () => {
  assert.deepEqual(await methods([`${TS}  WARN holon::api: slow\n`]), ['warn'])
})

test('an ERROR record is an error', async () => {
  assert.deepEqual(await methods([`${TS} ERROR holon::api: write lost\n`]), ['error'])
})

test('continuation lines of a multi-line ERROR stay errors', async () => {
  const out = await methods([
    `${TS} ERROR holon::api: bad DDL:\n  "id" TEXT PRIMARY KEY,\n  "email" TEXT,\n`,
  ])
  assert.deepEqual(out, ['error', 'error', 'error'])
})

test('continuation lines of a multi-line INFO stay info', async () => {
  const out = await methods([`${TS}  INFO holon::api: CREATE TABLE x (\n  "id" TEXT\n`])
  assert.deepEqual(out, ['info', 'info'])
})

// The payload line names what actually panicked; if it inherits the latched
// INFO the gate sees a panic with no message.
test('a panic split across chunks is an error, payload included', async () => {
  const out = await methods([
    `${TS}  INFO holon::api: booted\n`,
    "thread '<unnamed>' panicked at crates/holon/src/lib.rs:10:5:\n",
    'assertion failed: engine_init re-entered with a different utc offset\n',
    'note: run with RUST_BACKTRACE=1 to display a backtrace\n',
  ])
  assert.deepEqual(out, ['info', 'error', 'error', 'error'])
})

test('a panic split across ticks is still an error', async () => {
  const out = await methods(
    [
      `${TS}  INFO holon::api: booted\n`,
      "thread '<unnamed>' panicked at src/lib.rs:1:1:\n",
      'the payload\n',
    ],
    { between: tick },
  )
  assert.deepEqual(out, ['info', 'error', 'error'])
})

// An emscripten/wasm abort carries no tracing level at all.
test('a wasm abort after boot chatter is an error', async () => {
  const out = await methods([
    `${TS}  INFO holon::api: booted\n`,
    'Aborted(OOM). Build with -sASSERTIONS for more info.\n',
  ])
  assert.deepEqual(out, ['info', 'error'])
})

// The WASI buffer flushes on a byte boundary, not a line boundary, so a level
// token can arrive in two pieces.
test('a level token split mid-word is not demoted', async () => {
  const out = await methods([
    `${TS}  INFO holon::api: booted\n${TS} ER`,
    'ROR holon::api: real failure\n',
  ])
  assert.deepEqual(out, ['info', 'error'])
})

// The same bytes, one event-loop turn apart. A sink that flushes partials on a
// timer classifies "…Z ER" as its own line and the ERROR reaches console.info.
test('a level token split ACROSS A TICK is not demoted', async () => {
  const out = await route(
    [`${TS}  INFO holon::api: booted\n${TS} ER`, 'ROR holon::api: real failure\n'],
    { between: tick },
  )
  assert.deepEqual(out, [
    ['info', `${TS}  INFO holon::api: booted`],
    ['error', `${TS} ERROR holon::api: real failure`],
  ])
})

// One tick per byte: the record must arrive as ONE line, not shredded.
test('a record delivered one byte per tick stays one line', async () => {
  const line = `${TS} ERROR holon::api: real failure`
  const out = await route([...line, '\n'], { between: tick })
  assert.deepEqual(out, [['error', line]])
})

test('a line split mid-word keeps its text intact', async () => {
  const out = await route([`${TS}  INFO holon::api: organizati`, 'on_id column\n'], {
    between: tick,
  })
  assert.deepEqual(out, [['info', `${TS}  INFO holon::api: organization_id column`]])
})

test('an INFO whose payload mentions ERROR stays info', async () => {
  const out = await methods([`${TS}  INFO holon::api: retrying after ERROR RuntimeError\n`])
  assert.deepEqual(out, ['info'])
})

test('an INFO arriving after a panic returns the sink to info', async () => {
  const out = await methods([
    "thread '<unnamed>' panicked at src/lib.rs:1:1:\n",
    'the payload\n',
    `${TS}  INFO holon::api: still logging\n`,
    '  a continuation of the info\n',
  ])
  assert.deepEqual(out, ['error', 'error', 'info', 'info'])
})

// Nothing has established a level yet: a link failure must not be swallowed.
test('output before any level is an error', async () => {
  const out = await methods(['LinkError: function import requires a callable\n'])
  assert.deepEqual(out, ['error'])
})

test('TRACE and DEBUG are debug', async () => {
  const out = await methods([`${TS} TRACE holon: t\n${TS} DEBUG holon: d\n`])
  assert.deepEqual(out, ['debug', 'debug'])
})

// A record whose newline never comes must surface eventually — but as a marked
// partial at the latched severity, never re-classified as a line of its own.
test('a stale partial is released at the latched severity, marked', async () => {
  const out = await route([`${TS} ERROR holon::api: truncated`], {
    settle: STALE_PARTIAL_MS + 150,
  })
  assert.deepEqual(out, [['error', `[partial line] ${TS} ERROR holon::api: truncated`]])
})

// Real boot case: the wasm writes a span header, opens the DB, then writes the
// rest — the fragment goes stale while the sink is still latched at error
// because nothing has established a level yet. Its own INFO token is whole, so
// it must be believed rather than reported as the session's first error.
test('a stale partial carrying its own level is released at THAT level', async () => {
  const frag = `${TS}  INFO di.create_backend_engine{db_path=`
  const out = await route([frag], { settle: STALE_PARTIAL_MS + 250 })
  assert.deepEqual(out, [['info', `[partial line] ${frag}`]])
})

// A worker that aborts and dies mid-write never gets its newline. These lines
// are complete and unambiguous — they lack only that newline — so the gate must
// still see them.
test('a truncated but complete abort is an error', async () => {
  const msg = 'Aborted(OOM). Build with -sASSERTIONS for more info.'
  const out = await route([`${TS}  INFO holon::api: booted\n`, msg], {
    settle: STALE_PARTIAL_MS + 250,
  })
  assert.deepEqual(out, [
    ['info', `${TS}  INFO holon::api: booted`],
    ['error', `[partial line] ${msg}`],
  ])
})

test('a truncated but complete panic is an error', async () => {
  const msg = "thread '<unnamed>' panicked at a.rs:1:1: the payload"
  const out = await route([`${TS}  INFO holon::api: booted\n`, msg], {
    settle: STALE_PARTIAL_MS + 250,
  })
  assert.deepEqual(out, [
    ['info', `${TS}  INFO holon::api: booted`],
    ['error', `[partial line] ${msg}`],
  ])
})

// KNOWN LIMIT, asserted so a future fix shows up as a test change rather than
// as silence. A stall longer than the stale window landing INSIDE the level
// token splits one ERROR record in two and demotes both halves: the released
// fragment carries no token to promote it, and the remainder continues at the
// latched level. Accepted because the wasm does not pause for seconds mid-token
// — the same bytes a tick apart are handled correctly (see the tick test above).
test('KNOWN LIMIT: a multi-second stall inside the level token demotes the record', async () => {
  const out = await route(
    [`${TS}  INFO holon::api: booted\n${TS} ER`, 'ROR holon::api: real failure\n'],
    { between: () => sleep(STALE_PARTIAL_MS + 250) },
  )
  assert.deepEqual(out, [
    ['info', `${TS}  INFO holon::api: booted`],
    ['info', `[partial line] ${TS} ER`],
    ['info', 'ROR holon::api: real failure'],
  ])
})

test('a stale partial after INFO does not become an error', async () => {
  const out = await route([`${TS}  INFO holon::api: booted\n`, 'Aborted(O'], {
    settle: STALE_PARTIAL_MS + 150,
  })
  assert.deepEqual(out, [
    ['info', `${TS}  INFO holon::api: booted`],
    ['info', '[partial line] Aborted(O'],
  ])
})

// Still-arriving bytes are not stale, however long the record takes in total.
test('a slowly-arriving record is not released early', async () => {
  const out = await route(
    [`${TS} ERROR holon::api: a`, 'b', 'c\n'],
    { between: () => sleep(STALE_PARTIAL_MS * 0.6) },
  )
  assert.deepEqual(out, [['error', `${TS} ERROR holon::api: abc`]])
})
