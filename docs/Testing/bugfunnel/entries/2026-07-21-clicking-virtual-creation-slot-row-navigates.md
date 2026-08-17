---
id: 2026-07-21-clicking-virtual-creation-slot-row-navigates
date: 2026-07-21
gap: PERCEPTION
secondary: COVERAGE
status: OPEN
summary: >-
  Clicking the virtual creation-slot row navigates to a non-navigable
  __virtual: id → blank main panel + breadcrumb banner (round-4 fresh-eyes;
  adjacent to the V4 phantom-row class)
source_line: 1066
---

## Bug

Clicking the virtual creation-slot row navigates to a non-navigable
__virtual: id → blank main panel + breadcrumb banner (round-4 fresh-eyes;
adjacent to the V4 phantom-row class)

## Missing piece

virtual ids should not be navigation targets; invariant candidate: click
targets resolve to navigable entities

## Remedy

OPEN (dogfood-round4; candidate)
