---
id: 2026-10-09-head-insert-run-underflows-sort-key
date: 2026-10-09
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  After about 128 inserts at the head of one sibling group, minting a sort key
  between two head keys panics (debug) or mints a key that sorts after both
  neighbours (release, "0080").
---

## Bug
Found by the LoroCreateAtIndex spike (code audit plus a compat probe that minted
60 200 keys through Holon's `gen_key_between` on two versions of
`loro_fractional_index`), not by an automated Holon test. Lane `fi-bump`.

## Root cause
`crates/holon-core/src/fractional_index.rs` `gen_key_between` minted keys with
crates.io `loro_fractional_index` 1.6.0. Each head insert lowers the first byte
(`80`, `7F80`, ... `0180`, `0080`, then `007F80`, ...). For a pair with a zero
prefix such as `between(007E80, 007F80)`, 1.6.0 subtracts below zero at its
`src/lib.rs:81`: a debug build panics with `attempt to subtract with overflow`,
a release build wraps and returns `"0080"`, which sorts after both neighbours.
The sibling order in SQL is then wrong. The loro fork's 1.13.0 returns
`"007E8180"`. Red log: `lane-logs/fi-bump-red.log` (both new tests panic at
`loro_fractional_index-1.6.0/src/lib.rs:81:22`).

## Missing piece
No test inserts more than ~40 times into one sibling group. The keystone runs
`sequential_strategy(1..40)` transitions per case
(`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs:74`), so it
cannot reach the 128-deep head run, and the unit tests of the key module only
minted a few keys at the head.

## Remedy
Holon's crates use `loro_fractional_index` 1.13.0 from the loro fork rev (root
`Cargo.toml` `[patch.crates-io]`), so one package serves Holon's sort keys and
Loro's tree positions. Two unit tests in `fractional_index.rs` pin it: a
200-insert head run with a key minted between every adjacent pair, and a
compat test against `src/testdata/fi_1_6_0_keys.txt` (keys recorded under
1.6.0: 1.13 mints the same bytes and orders 100 new keys among them). Not done:
the keystone cannot reach this depth without a seeded deep sibling group; a
vault built in release can already hold a wrong `"0080"`-style key, which the
fix does not repair.
