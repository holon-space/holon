---
id: 2026-10-03-iroh-enrollment-refusal-can-read-as-a-plain-io-failure
date: 2026-10-03
gap: ENVIRONMENT
secondary: null
status: FIXED
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

Measured. On a refusal the acceptor returned from `acceptor_enroll` with the error, which
dropped the enrollment `SendStream`; its drop finishes the stream when the connection has no
error yet (noq `SendStream::drop`). Only then did the caller run `conn.close(
ENROLLMENT_REFUSED_CODE, ..)`. The stream end could reach the dialer first: its ack read failed
with "stream finished early" while the connection had no close reason yet, so
`classify_enrollment_failure` left the failure untyped. A pause between verdict and close made
the forged-capability dial untyped every time.

## Missing piece

No test held the acceptor between its verdict and its close, so the order in which stream end
and close reach the dialer was left to scheduling.

## Remedy

`acceptor_enroll` closes the connection with the verdict's code while the enrollment stream is
still open, then returns the error; the dropped stream no longer sends an end, because the
connection already holds its error. The accept loop only logs. The property
`iroh_advertiser::tests::a_verdict_reaches_the_dialer_whatever_the_close_delay` draws the verdict
(roster refusal or enrollment failure) and a pause of 0 to 400 ms between verdict and close, and
asserts the dial is typed `EnrollmentRefused` exactly for a roster refusal. The registry row
`iroh-pairing-refusal-reads-as-io-failure` is deleted.
