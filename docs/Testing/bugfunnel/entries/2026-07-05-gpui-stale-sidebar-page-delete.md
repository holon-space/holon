---
id: 2026-07-05-gpui-stale-sidebar-page-delete
date: 2026-07-05
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  GPUI stale sidebar on page-delete
source_line: 850
---

## Bug

GPUI stale sidebar on page-delete

## Missing piece

keystone never deletes a page (`apply_mutation` filters `!is_page()`);
sidebar watch never a RefWatch

## Remedy

open
