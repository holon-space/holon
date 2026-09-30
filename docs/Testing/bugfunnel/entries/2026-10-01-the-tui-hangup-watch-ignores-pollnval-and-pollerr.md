---
id: 2026-10-01-the-tui-hangup-watch-ignores-pollnval-and-pollerr
date: 2026-10-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The TUI's terminal hangup watch ends only on POLLHUP; a stdin that reports
  POLLNVAL or POLLERR is polled forever.
---

## Bug
Found by the implementer of the dogfood follow-ups lane (code audit,
`lane-logs/dffu-r2-report.md`, Findings).

## Root cause
`frontends/tui/src/terminal_hangup.rs:35` tests only `POLLHUP` in
`revents`. A closed descriptor (`POLLNVAL`) or an error (`POLLERR`) on stdin
leaves the watch running, with no log line.

## Missing piece
No test closes or breaks stdin other than by a terminal hangup.

## Remedy
Open, not fixed.
