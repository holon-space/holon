---
id: 2026-09-30-a-deep-vault-keeps-the-org-sync-busy-for-minutes-after-boot
date: 2026-09-30
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  On a 10,000-block vault (10 docs x 50 chains x depth 20) the session stays
  busy for more than 14 minutes after its boot is quiescent: the org
  file-sync controller's home_by fold reads blocks one by one through the
  Turso actor while IVM maintenance writes, and the first owning-page read
  of the test never returns.
---

## Bug
Found by an automated test, `owning_page_cost_on_a_deep_vault`
(`crates/holon-integration-tests/tests/authority_owning_page_latency.rs`), in
the dogfooding phase 1 lane (Increment 1c). It was read as "a slow test that
needs a longer nextest cap". That reading is wrong:

- The boot and `wait_for_loro_quiescence` finish (`OWNING_PAGE_BOOT loro`
  126 s and 210 s, test profile, host load 40-60).
- The first `measure("loro one-read", …)` then does not finish within 1200 s
  under nextest (TIMEOUT), or within 14 minutes under `cargo test`.
- A `sample` of the test process 14 minutes after the boot line
  (`lane-logs/dogfood-inc1c-owning-sample.txt` in that lane): the test thread
  waits in `block_on`. One worker is near 100 % CPU. Its hot frames are
  `holon_orgmode::di::run_file_sync_controller` →
  `holon_api::live_data::home_by::process_diff` (`BlockHomeAuthority`) →
  `CacheBlockReader::get_block_authoritative` / `BlockRowMemo::get` →
  `TursoBackend::query_rows`. They are mixed with Turso
  `incremental::join_operator … commit`, `persistence::WriteRow::write` and
  btree insert or balance. So the org sync does per-block authoritative reads
  and IVM maintenance without end, and the owning-page read waits behind it.

## Root cause
Not established. Hypotheses, most likely first:
1. The `home_by` fold over a deep vault does one authoritative read for each
   block for each diff, so the cost is O(N) per change and O(N²) for the boot
   seed.
2. A write-back feedback loop: a render changes a projected row, which
   re-triggers the fold.
3. IVM maintenance of a matview that joins on depth does too much work per
   write.

## Missing piece
The test has no latency oracle that fails with a message about latency. Its
only failure is the runner's timeout, and that reads as "the test needs more
time". No gate runs it.

## Remedy
OPEN. First, take a longer sample with `HOLON_OWNING_PAGE_DEPTH` and
`HOLON_OWNING_PAGE_CHAINS` reduced, to see whether the busy phase grows with N
or N², then triage with `holon-diagnostics`. The test gets no nextest override
until it finishes.
