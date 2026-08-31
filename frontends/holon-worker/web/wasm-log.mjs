// The wasm writes ALL `tracing` output to WASI stderr, so a sink that maps
// stderr straight to `console.error` drowns real errors in INFO chatter and
// makes "zero console errors" unusable as a gate. Route by the level token
// `tracing_subscriber`'s fmt layer emits after the timestamp.
//
// stderr is a byte stream flushed on buffer boundaries, not a sequence of
// records: one write can carry several records, split a record mid-line, or cut
// a level token in half — and the halves can land in different event-loop
// ticks. So a partial line is held, not classified, for as long as it keeps
// growing or has been idle less than STALE_PARTIAL_MS. Classifying "…Z ER" as
// its own line is what demotes the ERROR it belongs to.
//
// KNOWN LIMIT, pinned by the test file: a stall of more than STALE_PARTIAL_MS
// landing INSIDE the ~5-byte level token still splits that one record and
// demotes it, because the released fragment carries no token to promote it and
// the remainder then continues at the latched level. Accepted — the wasm does
// not pause for seconds mid-token — and left visible rather than papered over.
//
// A line with no level of its own continues the record before it, which keeps a
// multi-line event on one severity. Panics and aborts carry no level, so they
// are matched explicitly and latch the sink to error — their payload and
// backtrace lines are the part that says what died. Until any level has been
// seen (link failures, early aborts) the sink stays on error.
//
// A partial that stops growing is a record whose newline is never coming (a
// truncated abort message). After STALE_PARTIAL_MS of silence it is released,
// marked as partial. It takes its own level ONLY if a whole level token is
// already present in it — a fragment cut before its token ("…Z ER") keeps the
// latched severity instead, which is the case that must never be re-classified.
// The window is generous because the wasm really does pause mid-record: it
// writes a span header, opens a database, then writes the rest.
//
// The contract this file owes its caller is pinned by ../test/wasm-log.test.mjs.
const LEVEL = /^\s*\S+Z\s+(TRACE|DEBUG|INFO|WARN|ERROR)\s/
const FATAL = /panicked at|^thread '|\bAborted\(|\babort\(|RuntimeError|LinkError|unreachable executed/

// A stale release judges an INCOMPLETE line, so only self-delimiting evidence
// can be trusted there: a whole level token, or a fatal marker that is already
// closed. `Aborted(OOM).` needs no newline to be unambiguous; `Aborted(O`, cut
// mid-message, could still turn out to be anything and keeps the latched level.
const FATAL_COMPLETE =
  /panicked at|^thread '|\bAborted\([^)]*\)|\babort\([^)]*\)|RuntimeError|LinkError|unreachable executed/

const METHOD = {
  TRACE: 'debug',
  DEBUG: 'debug',
  INFO: 'info',
  WARN: 'warn',
  ERROR: 'error',
}

export const STALE_PARTIAL_MS = 2000

export function wasmStderrSink(prefix) {
  let pending = ''
  let pendingSince = 0
  let method = 'error'
  let timer = null

  // Resolved per call, never captured: the console a worker starts with is not
  // always the one it logs through.
  const write = (...parts) => console[method](prefix, ...parts)

  const emit = (line) => {
    const level = LEVEL.exec(line)
    if (level) method = METHOD[level[1]]
    else if (FATAL.test(line)) method = 'error'
    write(line)
  }

  const onTimer = () => {
    timer = null
    if (!pending) return
    const idle = Date.now() - pendingSince
    if (idle < STALE_PARTIAL_MS) {
      timer = setTimeout(onTimer, STALE_PARTIAL_MS - idle)
      return
    }
    const line = pending
    pending = ''
    const level = LEVEL.exec(line)
    if (level) method = METHOD[level[1]]
    else if (FATAL_COMPLETE.test(line)) method = 'error'
    write('[partial line]', line)
  }

  return (...args) => {
    pending += args.join(' ')
    const lines = pending.split('\n')
    pending = lines.pop()
    for (const line of lines) emit(line)

    if (!pending) return
    // Every new byte resets the clock: a record still being written is not
    // stale, however many ticks it spans.
    pendingSince = Date.now()
    if (timer === null) timer = setTimeout(onTimer, STALE_PARTIAL_MS)
  }
}
