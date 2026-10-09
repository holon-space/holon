---
id: 2026-10-08-arm-consumes-the-previous-panic-record-unshown
date: 2026-10-08
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  On mobile, `panic_record::arm` marked the previous run's panic record as
  seen at the start of boot, so a boot that died before its bus existed
  (`boot_failed` → `exit`, an OS kill, a JNI abort) destroyed that record
  unshown, and a `boot_failed` death left no record of its own.
---

## Bug
Found by the verifier of the boot-always A1+A2 change d922394e
(`lane-logs/a12-fix-verify.md`, R1; probe
`zz_probe_crash_between_arm_and_install` in `lane-logs/a12-fix-verify/probe.log`).
That change moved `arm` to the start of the Android and iOS boot
(`frontends/gpui/src/mobile.rs`), far ahead of `install`.

Boot N panics and leaves a record. Boot N+1 arms, which moves the record to
`last-panic.seen.json` and only queues its condition, then dies in
`boot_failed` (`shared_platform()` is `None`, or `Runtime::new` fails) through
`std::process::exit(1)`. Boot N+2 shows nothing: boot N's record was consumed
unshown and boot N+1 wrote none. On Android there are no logs, so that record
is the only evidence the user ever gets.

## Root cause
`arm` (`crates/holon-frontend/src/panic_record.rs`, `take_previous`)
consumed the record when it queued its condition, not when a bus showed it.
`boot_failed` (`frontends/gpui/src/mobile.rs`) ends in `process::exit`, which
runs no panic hook.

## Missing piece
No test ran more than one boot in more than one process, and none died
between `arm` and `install` by anything but a panic. The keystone boots
through `install` only, in one process, so the mobile arm-before-bus window
and a non-panic death do not exist in its wiring.

## Remedy
`arm` moves the previous run's record into `unshown-panics/<n>.json`;
`install` emits them as one condition on the bus it returns, and they move
to the bounded history `seen-panics/<n>.json` only when a frontend calls
`panic_record::seen_on` for that bus. The GPUI window calls it on the frame
after one whose previous-run toast lay inside the viewport with the newest
crash site in its laid out headline
(`frontends/gpui/src/share_ui.rs` `previous_run_probe`); the TUI and the
standalone MCP server draw no conditions and never call it. A run that dies
before the user saw its bus therefore keeps the records of every run before
it, and the next bus shows all of them in its one previous-run condition.
`boot_failed` writes its own record through `panic_record::record_exit`.
Pinned by `crates/holon-frontend/tests/panic_record_boot_loop.rs` (child
processes: panic, `exit`, `boot_failed`-shaped exit, an exit after
`install`, then a bus) and
`frontends/gpui/tests/panic_records_seen_after_a_drawn_frame_windowed.rs`;
red on d922394e in `lane-logs/a12-fix3/red.log` and on 9d9ea0da in
`lane-logs/a12-fix4/red-boot-loop.log`.
