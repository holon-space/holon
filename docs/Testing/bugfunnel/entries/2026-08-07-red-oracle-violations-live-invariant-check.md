---
id: 2026-08-07-red-oracle-violations-live-invariant-check
date: 2026-08-07
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  The red "ORACLE VIOLATIONS (N) — live invariant check failed" banner is
  drawn over the macOS title bar
source_line: 1169
---

## Bug

(overnight dogfood-explorer, same session) **The red "ORACLE VIOLATIONS (N)
— live invariant check failed" banner is drawn over the macOS title bar**,
covering the traffic-light window controls (close/minimise/zoom survive only
as coloured smudges under the red fill) and the tab strip beneath them.
While any oracle violation is displayed the tab bar is unreadable and the
window controls are unreliable to hit; the banner's own "dismiss ×" sits
inside the same overlapped strip. The banner is otherwise excellent and did
its job — it is what surfaced the `split_block` latency rows above — so the
defect is purely its z-order/inset, not its existence.

## Root cause

overnight dogfood — the red "ORACLE VIOLATIONS (N) — live invariant check
failed" banner is drawn OVER the macOS title bar: it covers the
traffic-light window controls (close/minimise/zoom are left as coloured
smudges under the red fill) and the tab strip beneath them. While any oracle
violation is displayed the user cannot read the tab bar or reliably hit the
window controls, and the banner's own "dismiss ×" sits in the same
overlapped strip)

## Missing piece

Layout/overlay defect on a platform chrome region no headless harness
models; the banner's own tests can only assert it renders, not where it
lands relative to the OS title bar. Missing piece = a windowed layout check
that the disclosure banner's top inset clears the title-bar region, or
moving the banner below the tab strip.

## Remedy

OPEN 2026-08-07 — diagnosis only. Evidence: `shots/05.png`, `shots/07.png`.
