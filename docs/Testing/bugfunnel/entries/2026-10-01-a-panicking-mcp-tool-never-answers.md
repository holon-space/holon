---
id: 2026-10-01-a-panicking-mcp-tool-never-answers
date: 2026-10-01
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  When an MCP tool handler panics, the call gets no reply: the agent's call
  hangs instead of receiving an error.
---

## Bug
Found while the decision Inc 6 lane reproduced the star-line panic of
`2026-10-01-dense-patch-star-body-line-panics-the-plan`. The engine test drove
`dense_patch` through the MCP server; a `tokio-rt-worker` thread panicked at
`frontends/mcp/src/dense_patch.rs:733` and the call did not answer within 60 s
(`lane-logs/inc6rb5-red.log`, lines 675 and 709).

## Root cause
The MCP server (rmcp) does not turn a panic in a tool handler into an error
reply; the handler task ends and the request stays open. Not investigated
further.

## Missing piece
No test asserts that every MCP call answers. The engine harness now limits
each `dense_patch` call to 60 s (`PATCH_ANSWERS_WITHIN`), which reports the
hang as a failure of one case.

## Remedy
Open. The dense_patch panics found so far are fixed; the server-side class
(any panicking tool hangs its caller) remains.
