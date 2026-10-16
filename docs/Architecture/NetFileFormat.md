# The net file format

*Part of [Architecture](../Architecture.md). Status: the format is built
(`crates/holon-net/src/file.rs`, pinned by
`crates/holon-net/tests/net_file.rs`). Nothing loads it into the executor yet
— that is holon-space/holon-work#370.*

A **net file** is one YAML file that holds one whole runtime net: its places,
its transitions, the shell commands its firings run, and the probes that feed
it facts from outside. It is the input of the sans-IO executor in
`crates/holon-net/src/exec.rs`, which owns the marking and enforces the firing
axiom of [ADR 0032 §9](../adr/0032-petri-net-execution-semantics.md).

It is not the simulation net of `crates/holon-engine/src/yaml/net.rs`
([Engine.md](Engine.md), [Simulation.md](Simulation.md)). That one declares
transitions over bound token types, with Rhai preconditions and a simulated
`duration`, to search a plan. This one declares the places and arcs of a net
that actually runs, with real child processes and a real clock. The two
formats stay separate until something needs one net to be both.

The example file is `crates/holon-net/nets/work-runtime.yaml`: the net that
coordinates our own agent work (holon-space/holon-work#363).

## A worked example

Four agents may build at once, so the build slots are a pool of four
interchangeable tokens. A probe asks the flow board who is alive, once a
minute, and each line of its output becomes one fact.

```yaml
name: work-runtime
places:
  Agent:
    capacity: register
  Cpu:
    capacity: fifo
    pool: 4
  Waiting:
    capacity: fifo
  Job:
    capacity: single
  Stalled:
    capacity: single
transitions:
  StartJob:
    trigger: auto
    priority: 10
    consumes:
    - Waiting
    - Cpu
    inhibits:
    - Job
    produces:
    - Job
    on_failure: Stalled
    on_timeout: Stalled
    effect: run_job
    receiver: none
    timers:
      run: 2h
      abandon: 3h
      count_bound: 2
      backoff: 5m
effects:
  run_job:
    kind: shell
    command:
      argv:
      - bash
      - -lc
      - just gate
      working_dir: /Users/martin/Workspaces/pkm/holon
      env_allow:
      - HOME
      - PATH
      timeout: 2h
    log_path: target/net/{transition}/{subject}/{attempt}.log
    exit_codes:
      done:
      - 0
      failed:
      - 1
      - 2
      other: outcome_unknown
probes:
  agent_liveness:
    command:
      argv:
      - bash
      - -lc
      - flow agents --json
      working_dir: /Users/martin/Workspaces/pkm/holon
      env_allow:
      - HOME
      - PATH
      timeout: 30s
    interval: 1m
    facts_into: Agent
    subject_field: agent
```

`StartJob` takes one waiting agent and one build slot, refuses to start while
that agent already holds a `Job`, and runs `just gate` as a child process. Exit
code 0 settles the run `Done`, 1 and 2 settle it `Failed`, and any other code
— 137 from a kill, say — leaves the outcome unknown, where the receiver
decides (ADR 0032 §9.2). After two attempts the count bound stops it.

## Places

| Field | Required | Meaning |
| --- | --- | --- |
| `capacity` | yes | `single`, `fifo` or `register` (ruled in holon-space/holon-work#322) |
| `pool` | no | `N` interchangeable tokens instead of one per subject |

- `single` holds one token per subject and **refuses a second put loudly**. It
  is the default choice: a silent replacement is a lost update.
- `fifo` is a queue per subject — candidates, waiting work, a pool of slots.
- `register` holds one token per subject that a fact put **replaces**, for a
  value that is re-derived rather than accumulated: a hash, a probe's fact.
- `pool: N` needs `capacity: fifo`, because the N tokens queue. The format
  only declares the pool; the behaviour is holon-space/holon-work#372.

## Transitions

| Field | Required | Meaning |
| --- | --- | --- |
| `trigger` | yes | `auto` (the executor fires it) or `manual` (only a caller does) |
| `priority` | yes | the order among the producers of one place; higher fires first |
| `consumes` | no | input arcs that take the token |
| `reads` | no | input arcs that leave the token, and go stale when it changes |
| `inhibits` | no | input arcs that enable the transition only while the place is empty |
| `produces` | no | output arcs |
| `facts` | no | output arcs whose tokens are facts about the outside world |
| `on_failure` | no | the failure port: the place that gets a token when a run settles failed (ADR 0032 §9.1) |
| `on_timeout` | no | the timeout port: the place that gets a token when the `run` timer ends an unsettled run (ADR 0032 §9.1) |
| `effect` | no | the effect descriptor a firing runs; absent means a pure firing |
| `receiver` | with `effect` | `keyed`, `state_probe` or `none` — what an unknown outcome does (ADR 0032 §9.2) |
| `timers` | no | `run`, `abandon`, `count_bound` with `backoff` — at least one (ADR 0032 §9.1) |

A `facts` output commits even when a read input changed while the effect ran;
a `produces` output does not (ADR 0032 §9.2). "No X, in flight included" is
two inhibitor arcs: one on the place, one on `<T>.Running`.

Timers are process class and monotonic (ADR 0032 §9.1): `run` bounds one run,
`abandon` orphans an attempt that never reported, and `count_bound` counts
attempts with the first included, with `backoff` the wait before the next one.
A bound without a backoff is half a retry rule, so the two come together. A
`timers` block that declares no timer at all is a load error, not an empty
default.

A failed run returns its input to the place it came from (ADR 0032 §9.2), so
the two ports are how a net sees that failure: `on_failure` gets a token when
the run settles failed, `on_timeout` when the `run` timer ends a run that
never settled. Without them a failure is invisible to the net, so nothing can
be modelled on it.

`receiver` is what makes an unknown outcome resolvable (ADR 0032 §9.2), so a
transition with an `effect` declares one and a pure transition declares none.
`keyed` means the receiver dedups by occurrence id, so the effect runs again —
which needs a `count_bound`, and the format refuses a keyed receiver without
one. `state_probe` means the transition checks that the goal holds, and
records "the goal holds", not "I did it". `none` parks the token in a
disclosed dead letter for the user to resolve.

## Effect descriptors

One kind today: `kind: shell`, a child process group
(holon-space/holon-work#371).

| Field | Required | Meaning |
| --- | --- | --- |
| `command.argv` | yes | the program and its arguments; no shell is interposed |
| `command.working_dir` | yes | the child's working directory |
| `command.env_allow` | yes | the only host variables the child inherits |
| `command.timeout` | yes | the child is killed when it runs longer |
| `log_path` | yes | where one attempt's output goes |
| `exit_codes` | yes | `done`, `failed` and `other` |

`log_path` is a pattern over `{transition}`, `{subject}`, `{attempt}` and
`{occurrence}`. It **must** name `{attempt}`: otherwise a retry overwrites the
log of the attempt before it, and the evidence of the first failure is gone.

`exit_codes` is a total map. `done` and `failed` list codes, and `other` says
what every code they do not name means — `done`, `failed` or
`outcome_unknown`. A code whose meaning is unknown belongs in
`outcome_unknown`, where the receiver decides, never in `failed`. A child
killed by a signal has no exit code; the timers settle that run, not this map.

## Probe descriptors

A probe is a command the host runs on its own interval. Each line of its
stdout is one JSON object, and each object becomes one fact token in
`facts_into`, under the subject that `subject_field` names
(holon-space/holon-work#373).

| Field | Required | Meaning |
| --- | --- | --- |
| `command` | yes | as above |
| `interval` | yes | how long the host waits between two runs |
| `facts_into` | yes | the place the facts go into |
| `subject_field` | yes | the field of each JSON line that names the subject |

## Durations

`250ms`, `30s`, `5m`, `2h` — a whole number and one of `ms`, `s`, `m`, `h`.
Zero is refused. Canonical form is the largest unit that divides the duration
exactly, so `90s` stays `90s` and `120s` is written back as `2m`.

## The canonical form

The writer emits one form: block style, the authored order of every map,
durations as above. A file in that form is written back byte for byte, which
is what `net_file.rs` pins. Comments are not part of the format and do not
survive a write-back — that is why the net files carry no prose and this page
exists. The writer is for generated nets and for the round-trip test; no tool
rewrites a net file a human authored.

## Every refusal names what it refused

`NetFile::parse` returns a `NetFileError`, never a net with a field guessed
for it. There is no default that hides a missing field: an absent arc list
means no arcs of that kind, and every other absent field is an error.

- an unknown key, a missing field, an unknown `kind` — the message carries the
  key and the position
- `capacity` that is not one of the three words, naming the word it got
- `pool` outside a `fifo` place, or a `pool` of 0, naming the place
- an arc or a `facts_into` onto a place the file does not declare, naming the
  transition or probe, the field and the place
- an `effect` no descriptor declares, naming the transition and the effect
- a transition with no input arc at all, which would fire forever
- a `timers` block with no timer, a `count_bound` of 0, and a `count_bound`
  without a `backoff` or a `backoff` without a `count_bound`
- an `effect` without a `receiver`, a `receiver` without an `effect`, and a
  keyed `receiver` without a `count_bound`
- an `on_failure` or `on_timeout` port onto a place the file does not declare
- one exit code mapped twice, naming the effect and the code
- a `log_path` with an unknown placeholder, an unbalanced brace, or no
  `{attempt}`
- an empty `argv`, and a duplicated place, transition, effect or probe key

## What the format does not express yet

Each of these is a field a later card adds, not a silent default:

- **Token class.** A place carries no class field. The executor refuses
  anything but process class (`LoadError::ClassNotSupported` in
  `crates/holon-net/src/exec.rs`), so the loader of holon-space/holon-work#370
  declares process class for every place, and this runtime rebuilds its
  marking from its probes after a restart. Device class needs the device store
  of ADR 0032 §9.4 first. A later `class` is an optional field whose default
  is `process`, so it reads every file written today and needs no revision of
  this format.
- **Producer order.** `priority` is in the file because producers of one
  place are ordered (ADR 0032 §9.1), but `exec::TransitionDef` has no priority
  field. holon-space/holon-work#370 maps it onto the executor's producer
  ordering, and adds the field to the executor if it has none; the loader does
  not order the transitions itself.
- **Guards.** A transition carries arcs only. Whether a firing may happen is
  what the arcs say; there is no expression to evaluate.
- **Correlated arcs.** The derived net of ADR 0032 §2 binds one subject across
  several places; an arc here is a place name, and the subject comes from the
  token the executor matches.
