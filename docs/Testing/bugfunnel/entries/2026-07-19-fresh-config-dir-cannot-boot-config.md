---
id: 2026-07-19-fresh-config-dir-cannot-boot-config
date: 2026-07-19
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Fresh config dir with no `holon.toml` cannot boot: `load_config` (config.rs)
  fed `holon.toml` to premortem's `Toml::file` WITHOUT `.optional()`/first-run
  handling, so a MISSING file was fatal (`Config errors: … file not found:
  …/holon.toml`). Breaks true first-run UX and the documented `just
  live-verify` recipe (the dogfood agent had to hand-seed an empty
  holon.toml). Note: the sibling loader `HolonConfig::load_runtime` already
  treated a missing file as Default — only the premortem `load_config` path
  was fail-loud on absence
source_line: 803
---

## Bug

Fresh config dir with no `holon.toml` cannot boot: `load_config` (config.rs)
fed `holon.toml` to premortem's `Toml::file` WITHOUT `.optional()`/first-run
handling, so a MISSING file was fatal (`Config errors: … file not found:
…/holon.toml`). Breaks true first-run UX and the documented `just
live-verify` recipe (the dogfood agent had to hand-seed an empty
holon.toml). Note: the sibling loader `HolonConfig::load_runtime` already
treated a missing file as Default — only the premortem `load_config` path
was fail-loud on absence

## Missing piece

no boot/config test exercised first run (fresh dir, no holon.toml) through
`load_config`; existing config tests only covered `save_preference`,
`load_runtime`, and serialization — never the premortem pipeline's
missing-file branch

## Remedy

FIXED (2026-07-19): `load_config` now detects a missing `holon.toml`,
persists the built-in `HolonConfig::default()` via `save_config`, and logs
`tracing::info!("first run: created default config at …")`; a
PRESENT-but-malformed file still falls through to `Toml::file` and fails
loud (parse error surfaced, never defaulted). Pinned by
`first_run_creates_default_config_and_round_trips` (fresh dir → loads
defaults + writes file; second load reads back identically, file unmodified)
and `malformed_config_fails_loud` (garbage TOML → `Err`, not fallback) in
`crates/holon-frontend/src/config.rs`
