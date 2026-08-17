---
id: 2026-07-19-fresh-config-dir-fails-boot-confusing
date: 2026-07-19
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Fresh config dir (no `holon.toml`) fails to boot with a confusing hard error
  instead of falling back to defaults (GPUI dogfood via `just live-verify`):
  the recipe creates the config DIR but no file, and `load_config`
  (`crates/holon-frontend/src/config.rs:354`) wires
  `premortem::sources::Toml::file(&toml_path)` which treats a MISSING file as
  a fatal parse error — boot aborts with `Config errors: ... file not found:
  .../config/holon.toml`. Hand-creating an empty `holon.toml` boots cleanly.
  First-run/onboarding robustness: a brand-new user (or the documented
  live-verify recipe) can't launch without an existing config file.
source_line: 1012
---

## Bug

Fresh config dir (no `holon.toml`) fails to boot with a confusing hard error
instead of falling back to defaults (GPUI dogfood via `just live-verify`):
the recipe creates the config DIR but no file, and `load_config`
(`crates/holon-frontend/src/config.rs:354`) wires
`premortem::sources::Toml::file(&toml_path)` which treats a MISSING file as
a fatal parse error — boot aborts with `Config errors: ... file not found:
.../config/holon.toml`. Hand-creating an empty `holon.toml` boots cleanly.
First-run/onboarding robustness: a brand-new user (or the documented
live-verify recipe) can't launch without an existing config file.

## Missing piece

keystone never exercises the config-load boundary with an absent
`holon.toml`; make the Toml source optional-on-missing (fall back to
Defaults) or have boot write a default config first, and add a
fresh-state-dir boot rung

## Remedy

OPEN — found GPUI dogfood 2026-07-19; workaround = pre-seed empty holon.toml
