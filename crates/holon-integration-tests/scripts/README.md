# iOS SUT reset (McpDriver keystone loop — plan Phase 1)

Host tooling for the `McpUserDriver` rung. Not compiled into the crate — it
drives the simulator from outside (`simctl`) while the driver connects to the
app over MCP. See `docs/Testing/McpDriver_KeystoneLoop_Plan.md`.

## `ios_reset_sut.sh` — per-case deterministic reset (Option B')

Resets the running Holon iOS-simulator app to a fixed, oracle-aligned seed:

1. `simctl terminate` the app
2. wipe the container's org root (`Documents/holon-pkm`) + SQL db (`Library/holon.db*`)
3. drop a fixed `:ID:`-drawered seed (so block ids match the oracle by
   construction; the non-empty org root suppresses `ios_data_paths`' default seeding)
4. `simctl launch` with the MCP port pinned
5. wait for MCP, then print `block_raw`'s sorted id-set (the Phase-1 exit check)

```sh
crates/holon-integration-tests/scripts/ios_reset_sut.sh \
    --udid <UDID> --bundle space.holon.gpui --port 8521 --seed <dir>
```

All flags optional: UDID falls back to `$IOS_SIM_UDID` then the booted sim;
bundle `space.holon.gpui`; port `8521`; seed `./seed_wide`.

## `seed_wide/` — the wide-tree seed

The remote face of `WIDE_TREE_ORG` (see `pbt::composed::wide_e2e`):

- `structural-page.org` — `structural-page` (Page-tagged doc, `sentinel:no_parent`)
  with `parent`/`c1`/`c2` as leaf siblings — the oracle's `wide_seed_tree`.
- `index.org` — the deterministic app shell (root-layout / sidebars / main-panel,
  fixed ids). Mirrors `DEFAULT_INDEX_ORG` in `frontends/gpui/src/mobile.rs`.
- `Journals.org` — a **date-free** `journals` page (the default seed's dated
  auto-entry is non-deterministic, so it is replaced).

Verified: two consecutive resets produce the identical 15-row `block_raw`
id-set — `structural-page` + `parent`/`c1`/`c2` (parented under the page) + shell
+ `journals`, with no date-based rows.

## Platform note

`simctl`-specific (iOS simulator). An Android equivalent would be a separate
`adb`-based script (different lifecycle/paths); only the MCP id-set probe is
shared, and Android GPU is currently non-functional in this environment, so it
is intentionally not built. A Rust wrapper belongs with the test entry (plan
Phase 5), gated `#[cfg(target_os = "macos")]`, not in the always-compiled lib.
