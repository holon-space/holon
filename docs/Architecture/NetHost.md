# The net host: netd

*Part of [Architecture](../Architecture.md). Status: built
(`crates/holon-netd`, pinned by `crates/holon-netd/tests/netd.rs` and
`crates/holon-netd/tests/netd_differential.rs`).*

`netd` is a long-running local process that hosts one or more standalone nets
([NetFileFormat.md](NetFileFormat.md)). For each net file it runs the sans-IO
executor behind the tokio shell of `crates/holon-net-shell`, starts the shell
effects the executor asks for, and runs the probes on their intervals.

```
netd --listen 127.0.0.1:7411 --state-dir ~/.local/state/netd nets/work.yaml nets/disk.yaml
```

## The endpoint

netd speaks MCP over streamable HTTP at `http://<listen>/mcp` and listens on
the loopback only: a `--listen` address off the loopback is refused. There is
no second protocol. The command line client is `mcpc`; a script calls the
JSON-RPC endpoint directly.

| Tool | Arguments | Answer |
| --- | --- | --- |
| `put` | `net`, `place`, `subject`, `content` (a JSON object) | the fact is in the place; the auto transitions it enabled have run |
| `fire` | `net`, `transition`, `subject`, `occurrence` | the manual firing is accepted under the caller's occurrence id |
| `marking` | `net` | every token of every declared place, and the conditions of the net and its probes |
| `why_not` | `net`, `transition`, `subject` | `enabled`, or the reason the executor gives |
| `await` | `net`, `occurrence` | the outcome once the occurrence settles: `done`, `failed`, `outcome_unknown` or `over_changed_inputs`, with its cause |

Every answer is plain text in the tool result's content and JSON in its
structured content (`mcpc --json`). A refusal is a tool error whose text
names the refused thing: the net, place, transition, subject or occurrence.

## Transitions without Rust

A net file holds no guards, so netd gives every transition the same logic:
the arcs alone decide whether it is enabled, and one firing puts one empty
token into each of its `produces` and `facts` places for the subject. An
effect transition runs its shell effect and maps the exit code as the
effect's `exit_codes` say; a run the runner kills on a timer has no outcome,
and the net's own timers settle it.

## No durable marking

The marking lives in memory only. On start, netd runs every probe once and
commits its facts before it listens, so after a crash or a restart the
marking is what the probes report and nothing else. Put facts and the tokens
of firings are lost on a restart; a net that must survive one keeps its
state where a probe reads it.

The shell effects of each net keep their process groups in
`<state-dir>/<net>.pid`. A start that finds groups of a netd that died still
running logs each of them and leaves them alone.

## Running it as a service

`crates/holon-netd/launchd/` holds a launchd agent template and its install
steps (`crates/holon-netd/README.md`).
