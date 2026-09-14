---
id: 2026-09-14-the-test-suite-files-share-secrets-in-the-developers-login-keychain
date: 2026-09-14
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Running the test suite on macOS raised the login-keychain authorization dialog
  again on every build and left share capability secrets in the developer's
  keychain, because the Loro wiring took the machine's keychain in tests too.
---

## Bug

Martin reported that the test suite pops the macOS login-keychain credential
dialog repeatedly and that "Always allow" does not stop it. Found by dogfooding
the build, not by any automated test.

Measured in his login keychain before the fix: 12 items written by test runs —
one `space.holon.owner-identity` / `founding-device` and eleven
`space.holon.share-capability` / `share/<uuid>`, the uuids being per-run fixture
share ids (`lane-logs/kc-names-before.txt`).

## Root cause

Two mechanisms, one cause: the machine's keychain was an ambient fact, so any
wiring could reach it.

1. `crates/holon-loro-wiring/src/loro_module.rs:554` built
   `ShareCredentials::platform()` unconditionally, so every headless harness
   that boots the Loro module held custody on the real keychain.
   `LoroShareBackend::install_roster` (`crates/holon-loro/src/loro_share_backend.rs:1242`)
   files the capability on every share creation, which is where the eleven
   `share/<uuid>` items came from.
2. The dialog returns on every build because a Rust test binary is ad-hoc signed
   with a per-build identity. Two builds of the same test target measured
   `Identifier=two_instance_composed_pbt-9e909325cc059e19` /
   `CDHash=adf753f9…` and `…-b8ae85bbd5f0b9d2` / `CDHash=cef0dedc…`,
   `Signature=adhoc`, `TeamIdentifier=not set` (`lane-logs/codesign-h1.log`).
   A keychain ACL trust entry is keyed on the applicant's code identity, so the
   "Always allow" granted to one build never matches the next binary.

The older guard (`holon_frontend::forbid_platform_keychain`) covered only
`FrontendSession`, so the share custody path bypassed it entirely.

## Missing piece

No invariant said "a test process never asks the machine for a credential", and
nothing structurally prevented a wiring path from resolving the platform store.
The keystone could generate the share interaction; it just had no oracle for
where the secret went, and the platform reach was invisible because a keychain
write looks like a successful save.

## Remedy

The machine's keychain became a capability. `holon_secrets::platform_keychain`
wraps the backend so every operation is refused unless a production `main`
called `grant_login_keychain`, and every attempt is counted. Test harnesses
inject `InMemoryKeychainStore` and `ShareCredentials::in_memory`, and the
composed catalog gained `inv-no-machine-keychain-access`, which asserts the
process's attempt count is zero.

The catalog entry runs on every composed case but cannot fail on this bug: no
drawn transition performs a keychain operation. What fails is
`assert_no_machine_keychain_access` in
`crates/holon-integration-tests/tests/two_instance_composed_pbt.rs`, which runs
the same invariant body at the `share_subtree` seam — the only path in the
keystone family that files a share secret. Closing that gap properly needs a
drawable transition that mints share credentials, which does not exist yet.

Red proof: with the injected share custody removed,
`production_pairing_refuses_a_receiver_that_holds_mounts` fails with
"refusing to store space.holon.share-capability/share/<uuid> … this process
never called `holon_secrets::grant_login_keychain`"
(`lane-logs/g3-twoinstance-red-*.log`). Restored, the target is green and the
keychain item census is unchanged across a whole-suite run.

The 12 pre-existing items in Martin's login keychain are NOT deleted by this
lane; see the lane report for the list by name.
