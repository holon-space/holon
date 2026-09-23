---
id: 2026-09-23-sync-sidecar-written-in-place-can-brick-the-boot
date: 2026-09-23
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The Loro sync-watermark sidecar was rewritten in place (truncate, then write),
  so a crash mid-write left a 0-byte or partial file, which the boot now refuses
  as undecodable.
---

## Bug
A fresh-context verifier found this by reading the code of the sidecar-durability
lane (report `scratchpad/fixlogs/fixlane-verify.md`, defect D1). Nobody saw it
happen in a running app.

## Root cause
`LoroProjection::persist_sidecar` (`crates/holon-loro/src/loro_sync_controller.rs`)
called `std::fs::write(&self.sidecar_path, bytes)`, which truncates the file and
then writes it. `load_sidecar_blocking` returns an error for bytes that
`Frontiers::decode` rejects, which includes 0 bytes and most prefixes.
`LoroModule` turns that error into a boot panic
(`crates/holon-loro-wiring/src/loro_module.rs`, `build the Loro projection`).
A crash or power loss between the truncate and the write therefore stopped the
next boot until someone deleted the sidecar by hand.

Unit test `loro_sync_controller::sidecar_tests::a_reader_never_observes_a_partial_sidecar`
had a reader run alongside 400 rewrites. On the in-place writer, 3 of 402 reads saw a
partial file (byte lengths `[0, 512, 0]`, expected 262144).

## Missing piece
Nothing checks what a crash leaves on disk between the two steps of a file
write. The keystone restart transitions only stop cleanly, and a crash while
the sidecar is being written is not modelled.

## Remedy
`write_sidecar` writes a temp file in the same directory with a unique name,
fsyncs it, renames it onto the sidecar, and then fsyncs the directory. The unit
test above failed on the in-place writer and passes on the atomic one.
