---
id: 2026-07-22-stack-overflow-abort-tokio-worker-unrecoverable
date: 2026-07-22
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  Stack-overflow abort (tokio-rt-worker, unrecoverable) during the
  boot/watch-subscription window under concurrent describe_ui+query on a clean
  28-page vault with NO id collision — intermittent (relaunch on same DB
  survives), so a DISTINCT non-deterministic trigger of the F8
  recursive-projection-overflow class (row 299 documents only the
  deterministic #+ID==:ID: self-parent cycle)
source_line: 1096
---

## Bug

Stack-overflow abort (tokio-rt-worker, unrecoverable) during the
boot/watch-subscription window under concurrent describe_ui+query on a clean
28-page vault with NO id collision — intermittent (relaunch on same DB
survives), so a DISTINCT non-deterministic trigger of the F8
recursive-projection-overflow class (row 299 documents only the
deterministic #+ID==:ID: self-parent cycle)

## Missing piece

recursive projection has no depth/cycle guard that fails loud; keystone
settle masks concurrent-boot-watch timing; F8 (row 299) guard would cover
both

## Remedy

open (extends F8/row 299; boot-overflow investigation lane in flight
2026-07-22)
