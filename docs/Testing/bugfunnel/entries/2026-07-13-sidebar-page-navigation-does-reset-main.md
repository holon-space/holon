---
id: 2026-07-13-sidebar-page-navigation-does-reset-main
date: 2026-07-13
gap: PERCEPTION
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Sidebar page navigation does NOT reset main-panel scroll: landing on a new
  page shows it mid-scroll (page title + first blocks above the fold,
  screenshot 03-genfile004.png); consequence over MCP: clicks on above-fold
  blocks fail "element bounds never committed" and focus is cleared
source_line: 976
---

## Bug

Sidebar page navigation does NOT reset main-panel scroll: landing on a new
page shows it mid-scroll (page title + first blocks above the fold,
screenshot 03-genfile004.png); consequence over MCP: clicks on above-fold
blocks fail "element bounds never committed" and focus is cleared

## Missing piece

no scroll-position-after-navigation assertion; keystone has no viewport

## Remedy

OPEN — dogfood #5, 800-block vault; expected: navigation scrolls to top
(LogSeq parity)
