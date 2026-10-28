---
id: 2026-10-28-a-shutdown-during-a-write-retry-names-no-unwritten-document
date: 2026-10-28
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  A shutdown whose settle deadline fell while a refused write-back's retry ran
  reported "Documents not written: []", so the user was not told which file
  misses edits that are only in the store.
---

## Bug
The verifier of the external-edit-lost lane saw one red of
`crates/holon-app/tests/shutdown_writes_back_every_edit.rs::a_refused_write_back_fails_the_shutdown_by_name`
in 21 runs: "shutdown: the org write-back did not settle within 20s ...
Documents not written: []". The refused document was not named. Possible data
loss: the user quits without knowing that a file lacks store edits.

## Root cause
- The executor removed the failure mark of a catch-up at its retry-due tick
  and set it again only when the re-fire failed (`crates/holon-net/src/exec.rs`,
  `timers`).
- `failed_writebacks` (`crates/holon-filesystem/src/writeback_net.rs`) read
  only the marks, and `crates/holon-app/src/session.rs` reads it once after the
  settle timeout. A deadline inside the window between the re-fire and its
  result gave an empty list.

## Missing piece
- Litmus COVERAGE: the shutdown test generates the refused write, but it hits
  the window only when its clock happens to stop in it; no test stopped the
  clock at the retry-due tick.
- Litmus ENVIRONMENT: the window is an async timing race between the shell's
  retry and the settle deadline, which the wall-clock test reaches by chance.

## Remedy
- The executor keeps the cause of the last failed attempt on the open run and
  exposes it as `Executor::retrying`; `failed_writebacks` names a document whose
  retry runs.
- Pinned by `writeback_net::tests::a_write_whose_retry_is_running_is_still_named_failed`,
  which stops the clock at the retry-due tick (red before, green after).
