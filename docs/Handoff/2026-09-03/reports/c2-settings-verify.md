# Adversarial verification — lane `c2-settings` (jj `vmuszsuu`, base `ed38a4dae833`)

Verdict: **REFUTED** — the unit-level behaviour is sound and every gate I re-ran is
green, but two defects stand: the headline user-facing promise does not hold on the
real desktop boot path (D1), and the claim-4 fix has zero test teeth (D2).

Workspace asserted on every command: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/c2-settings`.
`jj diff -r @ --stat` after all probes is byte-identical to the diff I started from.

---

## D1 (blocking) — a `shopping.list_url` stored in `holon.toml` never reaches the app

**Where:** `crates/holon-frontend/src/config.rs:390-421` (`load_config`, the desktop
boot path) vs `crates/holon-frontend/src/lib.rs:713`
(`preferences_render_data` reads `self.holon_config.lock().unwrap().preferences`).

**Reproduction (live, port 8720, throwaway profile `/tmp/holon-live-verify-c2`):**

1. `/tmp/holon-live-verify-c2/config/holon.toml` written at **21:25:26** containing

   ```toml
   [preferences]
   "shopping.list_url" = "https://shop.example/c/abc123SYNTHETICliveVERIFYq7Wv/api"
   ```

   (synthetic token; this is exactly the shape `HolonConfig::save_runtime`,
   `crates/holon-frontend/src/config.rs:606-648`, itself writes).
2. App launched at **21:25:41** — 15 s later, so the file was on disk at boot.
   `ps -o lstart= -p 9670` vs `stat` on the file.
3. Settings → Integrations → **Shopping List URL** paints **`Not set`**.
   Screenshot: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/c2-settings/lane-logs/live-A-settings.png`
4. `holon.toml` afterwards: sha256 unchanged, preference still present — the app
   did not overwrite it, it simply never read it.

**Second shape, same result:** `[preferences.shopping]` / `list_url = …` →
still `Not set`. Screenshot `lane-logs/live-B-settings-nested.png`.

**Control that isolates the leg (and shows it is NOT lane-introduced):**
`[preferences]` / `"ui.theme" = "holonDark"` — a key this lane does not touch — is
dropped identically; the Theme row still paints `holonLight (Light)`.
Screenshot `lane-logs/live-C-control.png`. So the whole
`HolonConfig.preferences`-from-disk read leg is dead on the desktop boot path;
`ui.theme` only *appears* to persist because `set_preference`
(`config.rs:657-670`) mirrors it into the typed `[ui] theme` field. `todoist.api_key`
and `shopping.list_url` have **no** typed mirror (the `_ => {}` arm at `config.rs:668`),
so they live only in the map that is dropped.

**Why this refutes claim 2 as a product claim:** `mcp_integrations.rs:550-555` builds
the resolver from `resolver.resolve::<HolonConfig>().preferences` — the same map. A
user who pastes the URL into Settings, sees it save, and restarts (the field is
`requires_restart: true`) gets an unresolved `${SHOPPING_LIST_URL}` and the sidecar
fails to build. Every test that "proves" claim 2 constructs the
`HashMap<PrefKey, toml::Value>` in memory
(`crates/holon-app/tests/settings_shopping_list_url_credential.rs:67-69`,
`crates/holon-frontend/src/integration_vars.rs:59-64`) and therefore cannot see this.

Base-attributed by the `ui.theme` control, but it makes the lane's headline
behaviour non-functional end to end.

## D2 — the claim-4 fix has no test teeth

`build_locked_display` (`frontends/gpui/src/render/builders/pref_field.rs:153-179`)
correctly masks by field TYPE. I inverted line 159 to the pre-fix behaviour
(`("secret", _) => value_str.to_string()`, i.e. paint the raw credential) and re-ran:

- `lane-logs/probe-gpui-rung.log` — `Summary [ 6.451s] 1 test run: 1 passed, 0 skipped`
- `lane-logs/probe-frontend.log` — `Summary [ 3.827s] 582 tests run: 582 passed, 0 skipped`

Both green with the hole reopened. Cause: the windowed rung never exports
`SHOPPING_LIST_URL`, so the shopping row is *unlocked* and goes through
`build_text_field`; no test renders a locked secret row. The unlocked mask does have
teeth (that assertion is what the rung pins).

Restored byte-for-byte; sha256 before and after both
`402cc2b2850113138fa557f8be1618b155ee0c3b98de1f2aa9c5e199104ac629`.

---

## Per-claim results

**1. Tree identity — CONFIRMED.** `grep -n "shopping.list_url" crates/holon-frontend/src/preferences.rs`
→ lines 201, 503, 507, 518, 554. Base `jj log -r @-` = `ed38a4dae833`.

**2. Resolver semantics — CONFIRMED as a unit; REFUTED end-to-end (see D1).**
Re-ran the name-mapping pin: `the_settings_key_and_the_sidecars_variable_are_the_same_name`
is in `lane-logs/verify-app-rung.log` → `Summary [ 0.014s] 6 tests run: 6 passed, 0 skipped`.
Probes, read off `integration_vars.rs:22-48`:
- *Different casing:* `PrefKey::new` (`preferences.rs:18-29`) accepts any alphanumeric,
  so `Shopping.List_URL` is constructible; `normalize_var_name` lowercases and maps
  `.`→`_`, so it still matches `SHOPPING_LIST_URL`. Collision hazard, not a break:
  `shopping.list_url` and `shopping_list.url` normalize to the same string and land in
  one `HashMap`, so whichever the iteration order visits last wins silently.
- *Empty preference:* filtered out at `integration_vars.rs:39`, so it is **not** "set".
- *Empty export:* filtered at line 45, falls through to the preference.
- *Both set:* the environment value is what `into_mcp_config_with` hands the sidecar.
- *Neither set:* `expand_vars` (`crates/holon-mcp-client/src/integration_config.rs:429-433`)
  returns `UnresolvedVar` — a loud failure, **not** an empty `base_url`. So an empty
  preference cannot silently produce a blank connector.

**3. Masked / never logged / redacted — CONFIRMED, one residual.**
- Redaction teeth: `the_pasted_token_is_registered_as_a_secret_and_never_reaches_an_error_string`
  (`settings_shopping_list_url_credential.rs:169-202`) carries its own negative control
  (an unregistered token must survive) and passes — `lane-logs/verify-app-rung.log`,
  `Summary [ 0.014s] 6 tests run: 6 passed, 0 skipped`.
- Windowed rung: `lane-logs/verify-gpui-rung.log`, `Summary [ 5.992s] 1 test run: 1 passed, 0 skipped`.
- *Could the leak check pass vacuously?* No for the unlocked path — the rung's positive
  control (`settings_integrations_ops_windowed.rs:279-283`) requires a painted
  `••••••••`, and `tracked_value` is what puts the row's text into the registry at all.
  Weakness: the control is not tied to the shopping row specifically; it would also be
  satisfied by any other masked row. In this fixture only shopping has a stored secret,
  so it holds today and is fragile to a fixture change.
- No logging path found: no `tracing`/`log`/`println!`/`dbg!` call takes `preferences`
  or a `HolonConfig` in `crates/` or `frontends/`. `HolonConfig` does `#[derive(Debug)]`
  (`config.rs:41`) and holds the raw map, so the hazard exists but nothing exercises it.
- **Residual:** `preferences_to_rows` (`preferences.rs:299`) puts the raw secret into
  the row's `value` prop. Masking happens only at paint time in the GPUI builder, so
  the cleartext credential is present in the view-model layer that render-tree
  introspection reads. Not a regression (todoist was the same), worth a decision.

**4. Locked + editable both mask, by TYPE not key name — CONFIRMED as code, REFUTED as covered.**
Locked: `pref_field.rs:158-162` matches on `pref_type == "secret"`. Editable:
`pref_field.rs:273-278`, same discriminant. Neither inspects the key. The pre-fix
`build_locked_display` printed `value_str` for any locked field — the diff confirms the
latent hole was real. Teeth: none (D2).

**5. `todoist.api_key` locks under an exported env var — CONFIRMED live, with a caveat.**
`TODOIST_API_KEY` is exported in the launch environment, and all three live runs paint
the Todoist row as `Set by CLI/environment` with a mask
(`lane-logs/live-A-settings.png`). Nothing writes the env value back: `holon.toml` sha256
was unchanged across three boots, and `build_locked_display` attaches no
`on_mouse_down`, so a locked row has no write path. *Caveat:* "reappears editable once
the env var is gone" cannot be demonstrated for a **stored** value, because D1 means a
stored value never reaches the row in the first place — it reappears as `Not set`.

**6. Live verification — DONE.** The recipe (`justfile:327`) takes **positional**
parameters: `live-verify port='8710' dir='/tmp/holon-live-verify'`. The correct call is
`just live-verify 8720 /tmp/holon-live-verify-c2`; the lane's `port=8720` is passed as
the literal *value* of the first positional. Ports 8710/8720/8730/8740 all free by
`lsof -i :<port> -sTCP:LISTEN -t`. Martin's pid 79729 on 8520 untouched (re-checked
after teardown). App launched, driven, and stopped; screenshots under `lane-logs/`:
`live-A-app.png`, `live-A-settings.png`, `live-B-settings-nested.png`, `live-C-control.png`.

*Operational hazard found while doing this:* `screenshot` with
`window_title: "Holon"` captured an **unrelated** window belonging to another
application — `select_window_index` (`frontends/mcp/src/tools.rs:4241-4254`) matches
the substring against every window on the machine and picks the largest, ignoring
`our_pid`. Omitting `window_title` filters by own pid and is correct. A screenshot tool
that can silently return someone else's screen is worth its own entry.

**7. Gates, re-run by me.** Toolchain `nightly-2026-08-16-aarch64-apple-darwin`
(`lane-logs/verify-toolchain.log`).

| Gate | Log | Result |
|---|---|---|
| `cargo fmt --all --check` | `lane-logs/verify-fmt.log` | empty (clean) |
| `cargo nextest run -p holon-frontend -p holon-mcp-client` | `lane-logs/verify-nextest-fe-mcp.log` | `Summary [ 7.669s] 924 tests run: 924 passed, 0 skipped` |
| `cargo check -p holon-gpui -p holon-app` | `lane-logs/verify-check.log` | exit 0, warnings only |
| `holon-app` credential rung | `lane-logs/verify-app-rung.log` | `Summary [ 0.014s] 6 tests run: 6 passed, 0 skipped` |
| gpui windowed rung | `lane-logs/verify-gpui-rung.log` | `Summary [ 5.992s] 1 test run: 1 passed, 0 skipped` |
| `just analyze-arch` | `lane-logs/verify-arch.log` | `archlint: 111 baselined violation(s) suppressed (see archlint/baseline.txt), 0 new violation(s).` |
| `just keystone-smoke` ×3 | `verify-keystone.log` / `2` / `3` | **1 RED, 2 green** — see below |

**keystone-smoke is flaky here, and the lane's green run proves little.**
Run 1 failed: `test result: FAILED. 3 passed; 1 failed; … finished in 80.33s`, panic at
`crates/holon-integration-tests/src/pbt/composed/harness.rs:1137`:

```
[inv-sql-budget] 1 budget violation(s):
  TypeChars.sql_reads: 25 dedup (raw 76, 51 redundant re-executions) exceeds expected 19 + tolerance 5 = 24
```

Runs 2 and 3: `test result: ok. 4 passed; 0 failed; … in 1.50s` / `2.51s`.
This is not in the known-reds list (those are the "cannot modify materialized view
block" reds), but it is a cross-cutting SQL-read budget with a margin of **1**, on a
code path this diff does not touch — base-attributed. I could not A/B against the base
rev without VCS writes, which the lane rules forbid.
Note the timing spread: `just pbt general 1` draws **one random sequence**, so the
lane's own `final-keystone-smoke.log` (`ok. 4 passed … in 2.72s`) exercised a trivial
sequence. A 2–3 s keystone-smoke is a near-vacuous gate result.

---

## What I did not do

- No fix of any kind (verifier role).
- No A/B of the keystone red against `ed38a4dae833` — needs a checkout.
- Did not exercise the settings write path through the app's own osascript dialog;
  D1 was established from the read side instead, with a lane-independent control.

---

# Re-verification — 2026-09-01, delta state of `vmuszsuu` (base still `ed38a4dae833`)

Verdict: **CONFIRMED.** D1 is fixed and proven live in both TOML shapes; D2 now has
teeth (red for the right reason). Two residuals are recorded below — neither blocks.
Tree after all probes: 14 files, `1125 insertions(+), 63 deletions(-)`; `pref_field.rs`
sha256 `402cc2b2850113138fa557f8be1618b155ee0c3b98de1f2aa9c5e199104ac629`, unchanged.

## (1) Both TOML shapes resolve — CONFIRMED, live and in a probe

Probe (temporary `crates/holon-frontend/tests/zz_verifier_probe.rs`, driving the real
`load_config` against real files on disk; run log `lane-logs/rv-probe.log`,
`test result: ok. 7 passed; 0 failed`; **file deleted afterwards**, `jj diff` back to 14):

| Input | `preferences` map after `load_config` |
|---|---|
| flat `"shopping.list_url"` + `"ui.theme"` | `{PrefKey("shopping.list_url"): String("FLAT"), PrefKey("ui.theme"): String("dracula")}` |
| nested `[preferences.shopping] list_url` | `{PrefKey("ui.theme"): …, PrefKey("shopping.list_url"): String("NESTED")}` |
| three segments, flat `"a.b.c"` | `{PrefKey("a.b.c"): String("THREE")}` |
| three segments, nested `[preferences.a.b] c` | `{PrefKey("a.b.c"): String("THREE_NESTED")}` |

`ui.theme` round-trips from disk in the map — the exact thing the old bug also killed.
Live confirmation: seeding `[preferences] "ui.theme" = "holonDark"` makes the Theme row
paint **`holonDark (Dark)`** (`lane-logs/rv-live-theme.png`); before the fix the same
input painted `holonLight (Light)`.

**Collision probe (both shapes for the same key in one file) — SILENT.**
`[preferences] "shopping.list_url" = "FLATWINS"` together with
`[preferences.shopping] list_url = "NESTEDWINS"` loads without error and yields
`{PrefKey("shopping.list_url"): String("NESTEDWINS")}` — one value wins by map-insert
order, with no diagnostic. `collapse` (`crates/holon-frontend/src/preferences.rs`,
`deserialize_preferences`) uses a bare `out.insert`, so a duplicate key overwrites
silently. Against the repo's "fail loud, never fake" rule this is the one place the new
deserializer degrades quietly. Low likelihood (the app only ever writes one shape), so
**residual, not blocking** — an `if out.insert(...).is_some() { return Err(...) }` would
close it.

## (2) Malformed key no longer panics the desktop boot — CONFIRMED, with a caveat

`load_config` now returns `Err`, surfaced by the caller as a `Result`:

```
MALFORMED -> Err: Config errors: Configuration errors (1):
  [merged config] '(root)': expected holon_frontend::config::HolonConfig, got "(complex)": Invalid preference key: "bad key!"
```

The message names the offending key — loud and actionable.

**Caveat:** `HolonConfig::load_runtime` (`crates/holon-frontend/src/config.rs:558`) still
`panic!`s on the same input — probe printed `LOAD_RUNTIME malformed panicked = true`,
`panicked at crates/holon-frontend/src/config.rs:558:21: Failed to parse …/holon.toml`.
That is the MOBILE boot path (`load_runtime_with_platform_overrides`), untouched by this
delta. Pre-existing, out of the lane's scope, worth a follow-up: a bad key typed on
desktop and synced to a phone aborts the phone's boot.

## (3) D2 teeth — CONFIRMED red for the right reason

Inverted `frontends/gpui/src/render/builders/pref_field.rs:159` to
`("secret", _) => value_str.to_string()`:

```
Summary [   5.691s] 1 test run: 0 passed, 1 failed, 0 skipped
panicked at frontends/gpui/tests/settings_integrations_ops_windowed.rs:295:13:
the settings window painted a credential: "https://shop.example/c/abc123SYNTHETICwindowedTOKENq7Wv/api"
 — neither a stored secret nor an exported one may reach the screen
```

Log `lane-logs/rv-d2-red.log`. Previously this same inversion left the rung green. The
rung now renders a locked secret row, so the locked path is covered. Restored
byte-for-byte, sha256 before and after both `402cc2b2850113138fa557f8be1618b155ee0c3b98de1f2aa9c5e199104ac629`.

## (4) Live masked-row evidence — CAPTURED (the lane's blocker was avoidable)

Booted on **8730** (free per `lsof -i :8730 -sTCP:LISTEN -t`), throwaway profile
`/tmp/holon-live-verify-c2b`, launched with `env -u SHOPPING_LIST_URL` so the preference
had to stand on its own. Settings → Integrations, **Shopping List URL**:

| Seeded shape | Painted | Screenshot |
|---|---|---|
| flat `"shopping.list_url" = "…SYNTHETICreverifyFLAT…"` | `••••••••`, editable (no "Set by CLI/environment") | `lane-logs/rv-live-flat.png` |
| nested `[preferences.shopping] list_url = "…SYNTHETICreverifyNEST…"` | `••••••••`, editable | `lane-logs/rv-live-nested.png` |

Before the fix the same seeding painted `Not set` (`lane-logs/live-A-settings.png` from
the first pass). Both runs also show Todoist as `Set by CLI/environment` + mask, so the
locked and unlocked secret renderings appear side by side in one frame.

The lane's stated blocker — `screenshot` cannot target the window — is not a blocker:
**omit `window_title`** and `select_window_index` (`frontends/mcp/src/tools.rs:4256-4267`)
filters by the server's own pid, which is the app. Passing `window_title: "Holon"` is
what captures a foreign window. `click` needs raw `x`/`y` (logical px); the gear sits at
`x ≈ window_width_px / 2 - 74`, `y = 19`.

## (5) Gates — all green

| Gate | Result | Log |
|---|---|---|
| `cargo fmt --all --check` | clean (0 bytes) | `lane-logs/rv-fmt.log` |
| `cargo nextest run -p holon-frontend -p holon-mcp-client -p holon-app` | `Summary [  22.960s] 1076 tests run: 1076 passed, 1 skipped` | `lane-logs/rv-nextest.log` |
| `just analyze-arch` | `archlint: 111 baselined violation(s) suppressed (see archlint/baseline.txt), 0 new violation(s).` | `lane-logs/rv-arch.log` |
| `/usr/bin/python3 scripts/bugfunnel.py check` | `585 entries, 0 problems` | `lane-logs/rv-bugfunnel.log` |
| `just keystone-smoke` ×3 | `ok. 4 passed; 0 failed` (8.68s / 2.51s / 2.03s) | `lane-logs/rv-keystone-1..3.log` |

Keystone was 3/3 green for me this round. Across the two passes the tree has produced
three different reds — my earlier `inv-sql-budget`, the lane's `inv-drawer-open-matches-ref`,
and now none — on invariants unrelated to this diff. Flaky, base-attributed; I agree with
the lane's reading. Note `just pbt general 1` draws ONE random sequence, so a 2 s green is
weak evidence either way.

Bugfunnel entry `docs/Testing/bugfunnel/entries/2026-09-01-dotted-preference-keys-split-by-config-layering.md`
is present and passes `check`.

## Residuals carried forward (neither blocks landing)

1. **Duplicate-shape collision is silent** — see (1).
2. **A theme in the preferences map is displayed but not applied.** The probe printed
   `FLAT typed ui.theme field = None`: `deserialize_preferences` fills the map, but the
   typed `config.ui.theme` field stays `None`, and the live boot painted the *light*
   chrome while the Theme row correctly read `holonDark (Dark)`. Harmless in practice —
   `set_preference` (`config.rs:657-670`) mirrors `ui.theme` into the typed field, so the
   app's own writes populate both — but a hand-edited `holon.toml` shows a theme it is
   not using. Pre-existing, untouched by this delta.
3. Unchanged from the first pass: `preferences_to_rows` (`preferences.rs`) still puts the
   raw secret into the row's `value` prop; masking happens only at paint.

---

# Round 3 — 2026-09-01, `vmuszsuu` with `PreferencesProbe` (base `ed38a4dae833`)

Verdict: **CONFIRMED** on R1/R2/R3. Two fragilities found in the probe's error
classification, neither a swallow. Tree after probes: 15 files,
`1386 insertions(+), 90 deletions(-)`; temporary probe test deleted.

## R1 — duplicate-key refusal on the desktop path

Probe (temporary `crates/holon-frontend/tests/zz_verifier_probe3.rs`, real
`load_config` against real files; `lane-logs/r3-probe.log`,
`test result: ok. 6 passed; 0 failed`; **file deleted**, `cargo fmt` re-run clean
afterwards at `lane-logs/r3-fmt2.log`, 0 bytes).

**(a) The duplicate is refused, and the key is named.** Input: `[preferences]`
`"shopping.list_url" = "FLAT"` plus `[preferences.shopping] list_url = "NESTED"`.

```
DUP -> Err("Failed to parse …/holon.toml: TOML parse error at line 1, column 1
preference "shopping.list_url" is set twice in holon.toml — once as a dotted key … and once as a nested table (the earlier value was "NESTED"). Keep one of the two.")
```

**(b) The probe cannot disagree with the real deserializer — same function.**
`HolonConfig.preferences` and `PreferencesProbe.preferences`
(`crates/holon-frontend/src/config.rs:441-447`) both carry
`deserialize_with = "crate::preferences::deserialize_preferences"`, so there is one
implementation, not a copy. Empirically identical: a direct
`toml::from_str::<HolonConfig>(DUP)` produces the **same** message the probe reports
(`DUP direct toml::from_str -> Err(… preference "shopping.list_url" is set twice …)`).

**(c) Non-preference errors are still premortem's, not swallowed.**

| Input | Reported by | Message |
|---|---|---|
| `storage = ` (syntax) | premortem | `Config errors: … parse error: string values must be quoted, expected literal string at line 1, column 11` |
| `bogus_top_level = 1` | premortem | `Config errors: … unknown field 'bogus_top_level', expected one of 'db_path', 'vault', …` |

**(d) No false positives.** Valid flat, valid nested and a plain `storage = "turso"`
file all return `Ok(())`. A malformed key is still refused
(`BADKEY -> Err(… Invalid preference key: "bad key!")`).

### Fragility 1 — the "preference" filter matches the ECHOED SOURCE LINE

`PreferencesProbe::check` classifies by `e.to_string().contains("preference")`
(`config.rs:434,458`). A `toml` error's `Display` includes the offending source line,
so the substring can match text that is not the complaint. Demonstrated: input
`preferences = 5` is claimed by the probe —

```
PREFS_SCALAR -> Err("Failed to parse …/holon.toml: … 1 | preferences = 5 … invalid type: integer `5`, expected a map")
```

— even though `invalid type: integer 5, expected a map` is not a preference-key
complaint; it matched on the echoed `preferences = 5` line. Consequence: any TOML error
occurring on a line containing the word "preference" (a syntax error inside the
`[preferences]` table, or a comment mentioning it) is reported by the probe rather than
premortem, losing premortem's source tracing. **Loud either way — a mislabel, not a
swallow.** The reverse risk does not exist: every `PrefKey` complaint
(`Invalid preference key: …`, `preference "…" is set twice …`) contains the word by
construction. Matching on a typed error instead of a rendered string would close it.

### Fragility 2 — the duplicate message renders with a run of spaces

`crates/holon-frontend/src/preferences.rs:104` wraps the format string across source
lines, so the user sees `…once as a dotted key                          and once as a
nested table…`. Cosmetic; the message is otherwise accurate and actionable.

### Unrelated residual surfaced by the same probe

`storage = 42` loads **successfully** (`WRONG_TYPE -> Ok(())`) — neither the probe (no
"preference" in the message) nor premortem rejects a wrong-typed scalar. Pre-existing
premortem lenience, outside this lane; noted because it sits next to the
`deny_unknown_fields` guard that *does* fire.

## R2 — `load_runtime*` now return `Result`; every caller surfaces it loudly

Repo-wide there is exactly **one** caller (grep over all `*.rs`/`*.kt`/`*.swift`
outside `target/`, excluding the defining file):

- `frontends/gpui/src/mobile.rs:153-159` — the mobile (iOS/Android) boot:
  `HolonConfig::load_runtime_with_platform_overrides(…).unwrap_or_else(|e| panic!("Cannot boot: {e:#}"))`.

No `.ok()`, no `unwrap_or_default()`, no silent fallback anywhere; `{e:#}` prints the
full anyhow chain, so the malformed-key text reaches the crash. This closes the caveat I
raised in round 2 (`load_runtime` panicking from inside the loader): the failure is now
a decision the boot site makes explicitly, with the message intact.

## R3 — reader enumeration spot-check: the TUI claim holds

`preferences_to_rows` (`crates/holon-frontend/src/preferences.rs:299`) emits `value`;
`preferences_render_expr` (`preferences.rs:428-448`) passes only literal `key`,
`pref_type` and `requires_restart`. **Nothing anywhere sets a `current` prop for a
`pref_field`.**

`frontends/tui/src/render/mod.rs:889` reads exactly that:

```rust
let current = node.prop_str("current").unwrap_or_default();
let line = format!("{}: {}", label, current);
```

So the TUI paints `"<label>: "` with an empty value for every preference, secret or not.
It also never reads `pref_type`, so it carries no masking branch at all. **Inert, not
leaking** — it renders nothing rather than rendering a credential. Claim confirmed.

The other two readers: `frontends/gpui/src/lib.rs:1075` (the real UI, masks by type) and
`crates/holon-frontend/tests/settings_secret_field_render.rs:51` (asserts on props).

## Gates

| Gate | Result | Log |
|---|---|---|
| `cargo fmt --all --check` | clean (0 bytes) after the probe file was removed | `lane-logs/r3-fmt2.log` |
| `cargo check --workspace --all-targets` | 0 `error` lines; finished with dependency future-incompat warnings only | `lane-logs/r3-check.log` |
| `cargo nextest run -p holon-frontend -p holon-app` | `Summary [  26.448s] 742 tests run: 742 passed, 1 skipped` | `lane-logs/r3-nextest.log` |

The first `fmt` run in this round failed on my own temporary probe file only
(`Diff in …/zz_verifier_probe3.rs` ×4, no other file); re-run after deletion is clean.
