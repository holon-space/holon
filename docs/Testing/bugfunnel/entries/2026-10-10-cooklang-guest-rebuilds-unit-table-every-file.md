---
id: 2026-10-10-cooklang-guest-rebuilds-unit-table-every-file
date: 2026-10-10
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  The Cooklang wasm guest rebuilt its unit tables on every parsed file, so a
  435-byte recipe spent 7.7 M fuel of which 5.25 M was setup.
---

## Bug
Martin found the 1 G fuel cap per guest call far too high for `.cook` files
(ruling COOK-fuel.c on holon-space/holon-work#108). A diagnosis lane measured
where the fuel goes instead of raising the cap.

## Root cause
- `parse_recipe` called `parser()` per file, which builds the cooklang
  `Converter` from the bundled and German unit files
  (`guests/cooklang/src/lib.rs`). That costs 5.25 M fuel; a real recipe then
  costs only about 2 M more.
- The default wasm allocator (dlmalloc) spends about 2.3 k fuel per small
  allocation and free; talc spends about 1.2 k. The parse allocates per token.
- Fuel overstates the work: wasmi charges a whole function or loop body on
  entry (only `loop`, `if` and `else` start a new fuel block), so cold `match`
  arms are paid on every iteration. An empty loop iteration in the guest
  costs 930 fuel.
- Measured on base main (fuel is profile-independent): pancakes.cook 7.74 M,
  200-step recipe 136.8 M, linear in the number of steps.

## Missing piece
`fuel_and_memory_headroom` asserts only a 4x margin under the cap. No test
bounded what one file costs on a warm host, so per-call setup was invisible.

## Remedy
- Build the parser once per guest instance (`thread_local`), and use talc as
  the guest's global allocator.
- After: pancakes.cook 1.91 M, 200-step recipe 104.9 M on a warm host.
- `a_warm_host_spends_fuel_on_the_recipe_rather_than_on_setup`
  (`crates/holon-plugin-host/tests/vault_scan_latency.rs`) pins both bounds;
  it was red on the base with 7 735 325 fuel for pancakes.cook.
