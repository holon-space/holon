---
id: 2026-07-18-automation-rule-card-overflows-right-screen
date: 2026-07-18
gap: PERCEPTION
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Automation-rule card overflows the right screen edge on the narrow Android
  display — no width constraint / wrap on the card, so its content runs
  off-screen (same on-device screenshot)
source_line: 805
---

## Bug

Automation-rule card overflows the right screen edge on the narrow Android
display — no width constraint / wrap on the card, so its content runs
off-screen (same on-device screenshot)

## Missing piece

no headless invariant can express layout overflow at a real device viewport;
the keystone has no narrow-viewport geometry rung — needs a windowed/layout
snapshot at Android width asserting card content stays within bounds

## Remedy

OPEN — found on-device 2026-07-18; fix = constrain/wrap the automation-rule
card width
