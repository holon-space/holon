---
id: 2026-08-07-hard-crash
date: 2026-08-07
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Hard crash: `thread 'tokio-rt-worker' (421026538) has overflowed its stack`
  / `fatal runtime error: stack overflow, aborting`.
source_line: 1166
---

## Bug

(overnight dogfood-explorer, same session) **Hard crash: `thread
'tokio-rt-worker' (421026538) has overflowed its stack` / `fatal runtime
error: stack overflow, aborting`.** The process aborted outright, taking the
window and the embedded MCP server with it (port dead, PID gone). NOT
REPRODUCED — one attempt, disclosed as unreproduced. Context at the abort:
immediately after the rapid-Enter burst rows above, during a `describe_ui` +
Escape sequence, with the memory monitor reporting RSS climbing 266.1MB →
301.3MB over the preceding minute. A stack overflow on a tokio worker
implies unbounded recursion rather than depth-of-data (the tree was 5
levels). Important companion result: relaunching against the SAME vault and
the SAME db booted cleanly and rendered correctly, so the persisted state is
NOT poisoned and the crash is not replayable from disk.

## Root cause

overnight dogfood — HARD CRASH, `thread 'tokio-rt-worker' (421026538) has
overflowed its stack / fatal runtime error: stack overflow, aborting`. The
process aborted outright, taking the window and the MCP server with it. NOT
REPRODUCED on one attempt — disclosed as such. Occurred after the rapid-edit
burst above, during a `describe_ui` + Escape sequence, with RSS climbing
266→301MB. Stack overflow on a tokio worker implies unbounded recursion.
Reassuring companion result: relaunching against the SAME vault booted
cleanly, so the on-disk state is not poisoned)

## Missing piece

Nothing exercises the GPUI app under interaction bursts that outrun
projection, which is the only state in which this was ever seen; and the
keystone runs no tokio worker with this call stack. Missing piece = first, a
reproduction (the recursion site is unattributed — candidates are the
recursive `focus_descendants` CTE consumer and the tree render/parent-chain
walks); a `RUST_MIN_STACK`-independent recursion guard that fails loud with
a depth would convert this from an abort into a diagnosable error.

## Remedy

OPEN 2026-08-07 — diagnosis only, UNREPRODUCED. Evidence:
`/tmp/dogfood-2026-08-07/logs/app-run1.log` (final two lines).
