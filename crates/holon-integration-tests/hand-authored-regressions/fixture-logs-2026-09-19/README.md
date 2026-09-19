# Fixture logs — 2026-09-19

Archived gate-log excerpts kept as evidence. Not read by any test; the
hand-authored harness loads only its sidecar JSONL, so this directory is inert.

- `journals-rewrite-race-parent-not-found-excerpt.log.zst` — transitions 7-9 of the
  hand-authored case `journals-external-rewrite-strips-convert-link-marks`, from the
  2026-09-18 landing gate (`land-w15b-landinggate-1789732107.log`, originally under
  `/tmp`, which does not survive a reboot). Transition 9/22
  `BlockToPage(journals::auto-create)` fails with
  `convert_block_to_page: constituent 'move_block' failed: Parent not found: block:7b512cac-d64f-e768-7a50-ac1e5a8f25f8`.
  Two things make this excerpt worth keeping. The failing page id is byte-identical to
  the one an injected projection lag reproduces on demand, so the derived page id is
  deterministic and both failures concern the same minted page. And the surrounding
  latency warnings show interactions expiring after 30+ seconds waiting for a delivered
  row, which is the Loro→SQL projection starved for far longer than the window the bug
  needs. See lane report `journals-rewrite-race` for the root cause and the reproducer.
