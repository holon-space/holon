---
id: 2026-08-31-wasm-mints-the-day-page-in-utc-not-the-viewer-zone
date: 2026-08-31
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  In the browser the `daily_journal` rule minted the day page on the UTC
  calendar date, so every viewer whose zone disagrees with UTC got the wrong
  journal page for part of each day.
---

## Bug

Driving the dioxus-web arm with the harness zone forced to `Pacific/Kiritimati`
(UTC+14), `web_arm_rule_engine_materializes_the_day_page` fails: the browser
engine holds a block titled `2026-08-31` while the viewer's calendar date is
`2026-09-01`. The `daily_journal` rule fired, so the rule engine is alive — it
simply minted the page on the wrong day. Found running the web PBT surface
(lane webpbt); red log `lane-logs/webpbt-tz-red.log`.

The same defect makes the day page appear late or early for a real user: in
CEST it is wrong between 00:00 and 02:00 local, and in every zone west of UTC it
is wrong for the last hours of the local day.

## Root cause

`Grain::sample` and `CalendarDate::from_clock` (crates/holon-api/src/clock.rs)
converted the clock instant with `chrono::Local`. wasm32 targets carry no tz
database, so there `chrono::Local` silently *is* UTC — there is no error, no
warning, and the value looks plausible. Native arms read the OS zone and agree
with their oracle, so only the browser diverges.

## Missing piece

The wasm engine had no channel through which the viewer's zone could reach it:
`Clock` carried an instant but no offset, so the calendar-date sites had to
reach for ambient `chrono::Local` and got UTC. Secondarily, the web arm's day
assertion is vacuous whenever the harness machine's zone happens to agree with
UTC on the calendar date — it compares two values that coincide for most of the
day, so it passed for two weeks against a build that had never been right.

## Remedy

`Clock` now carries `utc_offset_seconds`, and the two calendar-date sites build
a `FixedOffset` from it instead of consulting `chrono::Local`. `SystemClock` and
`TestClock` report the host offset per-instant (unchanged native behaviour, DST
boundaries included); the worker registers a `BrowserClock` whose offset the
page hands over at `engineInit` from `Date.getTimezoneOffset()`
(frontends/dioxus-web/src/main.rs, frontends/holon-worker/src/lib.rs).

**The lock is the unit test
`holon_api::clock::tests::calendar_date_follows_the_clocks_offset_not_the_host_zone`**,
which drives 23:00 UTC at offsets 0 / +14h / −8h. It is red against the
`chrono::Local` implementation for exactly this reason (`left: "2026-09-01",
right: "2026-08-31"` — `lane-logs/webpbt-clock-pin-red.log`). The env-forced
browser run `lane-logs/webpbt-tz-green.log` is corroboration, not the lock: it
only discriminates on a host whose zone disagrees with UTC.

The native side was vacuous too. `TestClock` took its offset from the host
zone, so every headless and native day-date assertion coincided with UTC
exactly as the web-arm one did. `TestClock` now carries an explicit offset
defaulting to a fixed +14:00, and the assertions that had silently depended on
the host zone (`from_clock_matches_local_date`, five `clock_scheduler` tests)
now name the zone they mean.

Still open: the offset is sampled once at boot, so a session left running
across a DST transition keeps the pre-transition offset until reload.
