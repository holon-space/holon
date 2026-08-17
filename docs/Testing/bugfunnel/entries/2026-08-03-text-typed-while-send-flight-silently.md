---
id: 2026-08-03-text-typed-while-send-flight-silently
date: 2026-08-03
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Text typed WHILE a send is in flight is silently destroyed when the ack
  lands: the `Ok(Ok(Delivery::Proven))` arm in
  `frontends/gpui/src/render/builders/input_box.rs` calls
  `draft.set(String::new())` unconditionally, so a user who starts composing
  the next message during the seconds-long dispatch loses it the moment the
  previous send proves. Inherited behavior — present before the compose-id
  lane, which left it unchanged (and now also re-mints the compose id in the
  same arm, so the clobbered draft silently inherits a fresh identity).
source_line: 1151
---

## Bug

(verifier code inspection during the compose-id lane) Text typed WHILE a
send is in flight is silently destroyed when the ack lands: the
`Ok(Ok(Delivery::Proven))` arm in
`frontends/gpui/src/render/builders/input_box.rs` calls
`draft.set(String::new())` unconditionally, so a user who starts composing
the next message during the seconds-long dispatch loses it the moment the
previous send proves. Inherited behavior — present before the compose-id
lane, which left it unchanged (and now also re-mints the compose id in the
same arm, so the clobbered draft silently inherits a fresh identity).

## Missing piece

No windowed test ever occupies the in-flight window: nothing types into the
input box between dispatch and ack. Missing: a windowed case that submits,
types new text while `send_state` is in flight, receives Proven, and asserts
the new text survives.

## Remedy

OPEN 2026-08-03 — diagnosis only. Fix direction: clear the draft only if it
still equals the submitted text (snapshot-compare at dispatch), keep the
re-mint unconditional.
