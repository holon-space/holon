---
id: 2026-10-03-iroh-enrollment-refusal-can-read-as-a-plain-io-failure
date: 2026-10-03
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  An unauthorized iroh dial can fail with "stream finished early" and no typed refusal even
  though the acceptor logged the refusal, so the dialer cannot always tell a refusal from an I/O failure.
---

## Bug

Found by lane runs of `two_instance_composed_pbt::production_pairing_replicates_the_whole_store_over_iroh`
(`lane-logs/reds-fix-4/full-core.log`, `lane-logs/reds-fix-4/novel-2.log`), not by a person.
The test dials a share with a wrong capability. In the failing runs the acceptor logs

`[advertiser:holon_tree] enrollment gate closed the connection (refused=true): enrollment
rejected: presented capability id does not match this share`

but the dialer reports `[init] enrollment did not complete: read enrollment ack: read enrollment
frame length: stream finished early (0 bytes read)`. The transport witness
(`two_instance_transport.rs`, the `Err(e) if !authorized` arm) accepts only the typed
`EnrollmentRefused`, so the test fails with "failed WITHOUT the acceptor refusing it". Measured:
1 of 3 isolated runs and 1 loaded full-core run failed; the other isolated runs passed.

## Root cause

Candidate, not measured. The dialer decides "refused" by reading `conn.close_reason()` right
after its ack read fails (`share_enrollment.rs`, `classify_enrollment_failure`). The acceptor
returns from `acceptor_enroll` with the error, and only then calls `conn.close(
ENROLLMENT_REFUSED_CODE, ..)` (`iroh_advertiser.rs`, the `Err(e)` arm). If the stream end reaches
the dialer before the connection-close frame, `close_reason()` is still `None` and the failure is
classified as plain I/O. Evidence for the candidate: the acceptor's `refused=true` line and the
dialer's untyped error occur in the same run.

## Missing piece

No test pins the order in which the refusal reaches the dialer. The classifier trusts a single
read of `close_reason()` at one instant.

## Remedy

OPEN. Candidate fix: the dialer waits for the connection's close (bounded) before it classifies a
failed ack read. It is a product change on a security signal and needs the `holon-feature`
red-first test. Until then the registry row `iroh-pairing-refusal-reads-as-io-failure`
classifies the shape.
