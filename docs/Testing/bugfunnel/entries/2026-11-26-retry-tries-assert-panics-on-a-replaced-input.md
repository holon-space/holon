---
id: 2026-11-26-retry-tries-assert-panics-on-a-replaced-input
date: 2026-11-26
gap: COVERAGE
status: FIXED
summary: >-
  A production assert forced the per-token tries header to equal the run's attempts; any replacement of a waiting retry's input token made the retry in the same run panic.
---

## Bug
- Found by the random part of the caller-input PBT (`no_sequence_of_caller_calls_panics`, crates/holon-net/tests/caller_inputs.rs) after the typed-token change had passed its verify and its gate.
- The tries assertion in `Executor::start` (crates/holon-net/src/exec.rs) panicked: "the P0 token of s counts 1 tries of 'T0', its run 2".
- One class, three paths, each a few calls. In each, T0 failed retryably and its run stays open:
  1. An outside write puts EQUAL content on T0's input. `Tokens::put_with` kept the version but reset the header.
  2. An outside write puts NEW content in a field that T0's bind does not read, before or during the retry. The mark key is equal (same input), so the run stays open on a token with an empty header.
  3. Another transition consumes the token T0 failed on, and an outside write brings a new token. The run stays open; the new token has no tries.
- All three paths are on main 5ab11bc7. Path 1 is red there as a direct test; paths 2 and 3 by reading (same `put_with`, `restore` and assert code).

## Root cause
- Two counters described one thing: `Run.attempts` per (transition, subject), which the count bound reads, and `Envelope.tries` per token, which nothing read.
- A production assert forced them equal. They cannot stay equal without patching every way a token is replaced or consumed while its run is open.

## Missing piece
- Coverage: the generator drew "replace the input of a transition whose retry waits" only rarely. It took a random seed long after the gate to reach path 1; paths 2 and 3 came from the next rounds.
- The assert guarded a redundant counter in production code, so each missed path was a panic instead of a wrong number nobody reads.

## Remedy
- `tries` is out of the token header until the retry step picks the unit of a retry (docs/Architecture/PetriNetLayers.md, "`tries` per token against attempts per run"). `Run.attempts` stays the only count; no behaviour change.
- An equal write leaves the held token unchanged (equal content is no change).
- The cases are static examples of the caller-input PBT, checked as "no panic, Level-dead, Crash law": `an_equal_rewrite_keeps_the_retry_count`, `new_unread_content_keeps_the_retry_count`, `a_write_during_the_run_keeps_the_retry_count`, `new_read_content_starts_a_new_run`, `a_retry_input_consumed_by_another_keeps_the_run`.
- The generator draws all three paths (`RetryInput`): a Keyed transition fails on one input; a second one is Keyed too or consumes the returned input with no effect; then an equal or a new-unread write.
- Keystone: the composed keystone PBT does not drive the holon-net executor with retryable failures. The caller-input PBT is the lowest layer that observes this behaviour.
