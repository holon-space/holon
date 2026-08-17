---
id: 2026-07-19-android-persisted-ignored-boot-changing-theme
date: 2026-07-19
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  ANDROID: the persisted `ui.theme` is IGNORED at boot — changing the theme in
  Settings persists correctly (`ui.theme = "dracula"` lands in the app-private
  `holon.toml`) and applies live, but after an app restart the app boots in
  LIGHT (`holonLight`) every time. Root cause: the mobile boot path built its
  config from `HolonConfig { db_path, vault, ..Default::default() }`
  (`frontends/gpui/src/mobile.rs:136`, since removed) and NEVER read the
  persisted `holon.toml` from the config dir, so `ui.theme` was always `None`
  → `load_theme_def` (`frontends/gpui/src/lib.rs:2726`) fell to its
  `holonLight` default. Desktop is NOT affected: it boots via
  `cli::build_session` → `config::load_config` (Defaults → holon.toml →
  CLI/env), which layers the persisted `[ui].theme` in (proven by
  `config::tests::load_config_applies_persisted_ui_theme`). This is the same
  platform-only boot divergence as the config-dir SIGABRT (row 223): the
  mobile embedder hand-builds `HolonConfig` instead of going through the
  shared load path.
source_line: 1022
---

## Bug

ANDROID: the persisted `ui.theme` is IGNORED at boot — changing the theme in
Settings persists correctly (`ui.theme = "dracula"` lands in the app-private
`holon.toml`) and applies live, but after an app restart the app boots in
LIGHT (`holonLight`) every time. Root cause: the mobile boot path built its
config from `HolonConfig { db_path, vault, ..Default::default() }`
(`frontends/gpui/src/mobile.rs:136`, since removed) and NEVER read the
persisted `holon.toml` from the config dir, so `ui.theme` was always `None`
→ `load_theme_def` (`frontends/gpui/src/lib.rs:2726`) fell to its
`holonLight` default. Desktop is NOT affected: it boots via
`cli::build_session` → `config::load_config` (Defaults → holon.toml →
CLI/env), which layers the persisted `[ui].theme` in (proven by
`config::tests::load_config_applies_persisted_ui_theme`). This is the same
platform-only boot divergence as the config-dir SIGABRT (row 223): the
mobile embedder hand-builds `HolonConfig` instead of going through the
shared load path.

## Missing piece

The headless keystone injects an explicit `HolonConfig` into the session and
never exercises the mobile embedder's boot-config construction
(`open_holon_window`), nor a save-preference→restart→re-read cycle — so a
mobile boot path that discards the persisted config is structurally
invisible to it. Now host-testable: the boot-config composition was lifted
into `HolonConfig::load_runtime_with_platform_overrides` (config.rs,
always-compiled) with tests asserting the persisted theme survives while
platform paths/CRDT still override; remaining gap = on-device
save-theme→kill→relaunch validation.

## Remedy

FIXED 2026-07-19. New
`HolonConfig::load_runtime_with_platform_overrides(config_dir, db_path,
vault_root, crdt_enabled)` (`crates/holon-frontend/src/config.rs`) starts
from `load_runtime` (persisted `holon.toml` → typed config;
first-run/no-file → built-in defaults; MALFORMED file panics loud, never a
silent default) then applies ONLY the non-preference platform overrides
(db/vault paths when the caller resolved one — `None` never clobbers a
persisted value; unconditional mobile CRDT opt-in).
`frontends/gpui/src/mobile.rs` `open_holon_window` now calls it instead of
`::default()`. TESTS (`crates/holon-frontend/src/config.rs`, host,
always-compiled): `mobile_boot_applies_persisted_ui_theme` (RED before fix —
persisted `dracula` was dropped),
`mobile_boot_first_run_defaults_theme_and_keeps_overrides`,
`load_config_applies_persisted_ui_theme` (desktop-scope control). Gate:
`cargo test -p holon-frontend --lib config::` 14/14 green (log
`~/.claude/jobs/00b6f50c/tmp/theme-boot-configtest.log`); `cargo ndk -t
arm64-v8a check -p holon-gpui --features mobile --no-default-features` for
the mobile.rs call site. On-device save-theme→restart validation deferred to
orchestrator.
