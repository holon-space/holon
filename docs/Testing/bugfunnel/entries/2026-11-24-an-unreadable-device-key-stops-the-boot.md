---
id: 2026-11-24-an-unreadable-device-key-stops-the-boot
date: 2026-11-24
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A device key that did not load (wrong length, unreadable store) panicked the
  Loro wiring's DI factory, so the app did not boot and the vault's shared pages
  could not be opened.
---

## Bug

The per-crate code review of `holon-loro-wiring` found `.expect` on the device
key load in the factory that builds the share backend. Found by a code audit,
not by any automated test.

## Root cause

The share backend took the device identity as a constructor argument, so the
key had to load before the backend existed. The factory unwrapped the load:
any entry that was not a 32-byte key panicked the DI resolve, and the boot
stopped. A red run on the custody revision shows the panic
`the device key is not available: Corrupt { len: 5 }` from the factory for a
five-byte entry.

## Missing piece

No boot test drew a device-key state before a reboot. The keystone has no
reboot transition, and the boot suite booted only over a freshly minted key,
so the unreadable-entry path never ran.

## Remedy

The key loads off the boot path in the device-key net
(`crates/holon-loro-wiring/src/device_key_net.rs`): an unreadable entry raises
a condition with a fix offer, sharing stays off with one disclosed error, and
shared content stays editable. `boot_suite::device_key_entry_boot` draws every
key-entry state × legacy key file × shares × edits × confirm × retry before a
second boot.
