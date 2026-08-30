---
id: 2026-08-31-gated-suite-guard-miscounts-single-test-binary
date: 2026-08-31
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  `check-gated-test-suites.sh` read every ONE-test gated suite as holding zero
  tests and failed it as dead — libtest's summary line is singular ("1 test, 0
  benchmarks") and the guard matched only the plural.
---

## Bug

Found while wiring the `web-arm`-gated suite into CI (ruling D47.a), not by any
test.

With the CI step added, `scripts/check-gated-test-suites.sh` accepted the
wiring — `ci: a workflow runs 'holon-integration-tests' with --features
web-arm` — and then failed the same suite on its other check:

```
FAIL: holon-integration-tests/web_arm_spike lists 0 tests WITH --features web-arm
      — the gated suite is dead (renamed feature / emptied file / tests moved out).
```

The suite is not dead. The identical command run by hand lists it:

```
web_arm_spike_drives_the_browser: test

1 test, 0 benchmarks
```

## Root cause

The count extraction matched the plural only:

```sh
count=$(printf '%s\n' "$list_out" | grep -oE '[0-9]+ tests' | ...)
```

libtest's `--list` summary is grammatically agreed: a binary with exactly one
test prints `1 test, 0 benchmarks`, not `1 tests`. The plural-only match found
nothing, `count=${count:-0}` defaulted to 0, and the guard reported the suite as
emptied.

Latent since the guard landed, because every other non-default-gated suite in
the tree holds 2 or more tests (the 18 `di`-gated `holon-orgmode` suites hold 2
to 33 each). `web_arm_spike` is the first one-test gated binary, so it is the
first draw that reaches the state.

## Missing piece

**ORACLE.** The guard read the right data and computed the wrong verdict from
it. Discovery is not the weakness — the guard found and compiled the suite
correctly; only the pass/fail judgement was wrong, and it was wrong in the
false-RED direction (a suite that IS alive reported as dead). The mirror-image
risk is worse and remains untested: the same defaulting (`count=${count:-0}`)
means any future change to libtest's summary wording degrades to 0, which fails
loud rather than passing silently, so the failure direction is at least safe.

## Remedy

Match either grammatical number, anchored on the summary line's comma so the
per-test lines cannot be picked up
(`scripts/check-gated-test-suites.sh`):

```sh
count=$(printf '%s\n' "$list_out" | grep -oE '[0-9]+ tests?,' | grep -oE '[0-9]+' | tail -1)
```

A genuinely empty binary still prints `0 tests, 0 benchmarks`, so the check it
exists for keeps firing.

Evidence, both runs against the same tree at
`target/gate-logs/webarm-ci-guard-*.log`:

* before — `FAIL: … lists 0 tests`, `check-gated-test-suites: FAILED`, exit 1;
* after — `ok: 1 tests` for `web_arm_spike`, `check-gated-test-suites: PASS —
  every non-default-gated test suite (19) is non-empty and CI-wired`, exit 0,
  with all 18 `di`-gated suites still reporting their previous counts.

### Residual

The guard has no self-test, so its own arithmetic is only exercised by the tree
it happens to check — this bug survived because no gated suite had one test.
A fixture pair (a one-test and a zero-test gated binary the guard must classify
correctly) is the durable answer and is not done.
