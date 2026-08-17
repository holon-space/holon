---
id: 2026-07-19-android-severe-changing-preference-theme-glass
date: 2026-07-19
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  ANDROID SEVERE: changing ANY preference (theme, glass background, API keys —
  the whole Settings modal) SIGABRTs the process — `signal 6 — 'Failed to
  create config dir .holon: Read-only file system (os error 30)'`. Chain:
  `PopupMenu::confirm` → `dispatch_set_preference` →
  `FrontendSession::set_preference` → `HolonConfig::save_runtime` `panic!` at
  `crates/holon-frontend/src/config.rs:534`
  (`create_dir_all(parent).unwrap_or_else(panic)`). TWO root causes: (1) PATH
  — on Android the CONFIG dir resolved via `resolve_config_dir(None)`'s
  relative `.holon` default, which lands under the read-only CWD `/`, while
  the vault/DB correctly used `android_activity`'s app-private
  `internal_data_path()`/`external_data_path()`; (2) FAIL-FATAL — a failed
  preference persist `panic!`ed (SIGABRT) instead of surfacing a visible
  degraded-mode error, violating "fall back VISIBLY, never abort the process
  for a preference write". Evidence:
  `~/.claude/jobs/00b6f50c/tmp/android-df-crash-full-tombstone.txt`.
source_line: 1021
---

## Bug

ANDROID SEVERE: changing ANY preference (theme, glass background, API keys —
the whole Settings modal) SIGABRTs the process — `signal 6 — 'Failed to
create config dir .holon: Read-only file system (os error 30)'`. Chain:
`PopupMenu::confirm` → `dispatch_set_preference` →
`FrontendSession::set_preference` → `HolonConfig::save_runtime` `panic!` at
`crates/holon-frontend/src/config.rs:534`
(`create_dir_all(parent).unwrap_or_else(panic)`). TWO root causes: (1) PATH
— on Android the CONFIG dir resolved via `resolve_config_dir(None)`'s
relative `.holon` default, which lands under the read-only CWD `/`, while
the vault/DB correctly used `android_activity`'s app-private
`internal_data_path()`/`external_data_path()`; (2) FAIL-FATAL — a failed
preference persist `panic!`ed (SIGABRT) instead of surfacing a visible
degraded-mode error, violating "fall back VISIBLY, never abort the process
for a preference write". Evidence:
`~/.claude/jobs/00b6f50c/tmp/android-df-crash-full-tombstone.txt`.

## Missing piece

Headless keystone never resolves platform storage dirs (it injects an
explicit writable config_dir), so it structurally cannot see a relative
config path landing on a read-only mount; and no test exercised the
`save_runtime` write-failure path, so the panic-on-failed-persist went
unnoticed. Host-side coverage now exists for BOTH the pure path-derivation
(`android_storage_paths`) and the Result path (`save_runtime` on an
unwritable parent); the remaining gap (a real read-only Android mount + live
Settings write) needs on-device validation.

## Remedy

FIXED 2026-07-19. (1) PATH — new host-testable
`android_storage_paths(internal, external)` in
`frontends/gpui/src/mobile.rs` routes CONFIG + DB → app-private INTERNAL
storage (`internal_data_path().join("config")` / `holon.db`) and the org
vault → EXTERNAL app-private (`external_data_path().join("holon-pkm")`);
`open_holon_window` gained a `config_dir: Option<PathBuf>` param that
`android_main` now pins from internal storage (iOS passes `None`, deferring
to the writable `$HOME`-based `resolve_config_dir`). (2) FAIL-LOUD-NOT-ABORT
— `HolonConfig::save_runtime` returns `anyhow::Result<()>` (no `panic!`);
`FrontendSession::set_preference` propagates it; a new
`BuilderServices::set_preference` fallible method +
`set_preference_or_toast` in `pref_field.rs` surface a red
`DegradedKind::PreferenceSaveFailed` toast on the GPUI toggle/choice paths;
`update_ui_settings` and the two `dispatch_intent`/`dispatch_intent_sync`
preference branches log-loud / propagate instead of aborting. TESTS:
`config::tests::save_runtime_unwritable_parent_returns_err_not_panic`
(read-only temp dir → Err, no panic) +
`save_runtime_persists_and_returns_ok`;
`mobile::tests::android_storage_paths_routes_config_to_internal_app_private`
+ `_passes_through_none`. Gates: `nextest -p holon-frontend --lib` 330/330;
`nextest -p holon-gpui --features mobile --lib android_storage_paths` 2/2;
`cargo check -p holon-gpui` (host) clean; `cargo ndk -t arm64-v8a check
--release --no-default-features --features mobile` clean. On-device
validation deferred to orchestrator (build APK + change a preference, assert
no SIGABRT and `config/holon.toml` lands under
`/data/data/space.holon.gpui/files/config`).
