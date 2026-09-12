---
id: 2026-09-12-an-introduced-connection-gets-no-settings-field-for-its-secret
date: 2026-09-12
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  The Settings secret fields are a compile-time list of two keys, so a
  connection a user introduces without a rebuild has no in-app way to enter its
  credential — the "no rebuild" promise stops at the secret.
---

## Bug

Found by the `dogfood-explorer` gate for `user-connections` (main
`f134df9ece6c`).

The feature's stated user path is: drop `<name>.yaml` into
`{config_dir}/integrations/`, switch it on, and its `${NAME_*}` secret is
resolved environment → keychain → plaintext preference, with the Settings field
showing a masked "stored in the keychain" placeholder.

Driven live with an introduced connection `fixturebox` referencing
`${FIXTUREBOX_TOKEN}`, enabled and connected: the Settings modal's Integrations
preference section contains exactly two fields — "Todoist API Key" and "Shopping
List URL". There is no Fixture Box field, before or after enabling.
Screenshots `scratchpad/dogfood-uc/shots/02-after-gear.png` and
`06-masked-secret.png`.

So a user who introduces a connection can switch it on from Settings but cannot
give it a credential from Settings. The only ways in are an environment variable
at launch or the `holon-secret` CLI, neither of which the Settings surface
mentions.

## Root cause

`crates/holon-frontend/src/preferences.rs:335-360`. The preference schema is a
hard-coded `Vec<PreferenceDef>` built in code. The two secret entries
(`todoist.api_key`, `shopping.list_url`) are literal `PrefKey::new(...)` calls
with literal labels, descriptions and `env_override` names. Nothing in that
builder consults the connection roster.

Presence became a union in Increment 4 — `ConnectionRoster::scan` unions the
bundle with installed files, and the store, the loader and the settings TABLE
all read that one scan. The preference schema was not part of that cutover, so
the roster and the secret-entry surface now disagree about which connections
exist.

The masking machinery itself is sound and was verified in the same session: with
a legacy plaintext value seeded into `holon.toml`, "Shopping List URL" changes
from "Not set" to `••••••••`, and the value appears in neither `describe_ui` nor
any `integration_state` row (0 matches for the synthetic token in both). That
half of Increment 5 works. It simply cannot apply to an introduced connection.

## Missing piece

COVERAGE. `crates/holon-frontend/tests/integration_secret_lookup.rs` covers the
environment → keychain → plaintext precedence for a key that EXISTS in the
schema. No test asks whether a connection outside the compiled list has a key at
all, because every fixture names one of the two compiled keys.

Secondary ENVIRONMENT: the lookup chain in production reads a var name derived
from the sidecar, while the settings surface reads a compile-time key list —
two different sources of truth for "which secrets exist", and no wiring test
compares them.

The keystone PBT cannot reproduce this; the preference schema is not part of its
state.

## Remedy

FIXED in lane `uc-fixes` (wave 12).

The design question the entry raises is answered by deriving the field and
naming its provenance, rather than by adding a disclosure that says "you cannot
set this here":

- `crates/holon-app/src/introduced_secrets.rs` (new) — every `${VAR}` the
  introduced connections in the integrations directory reference, with the file
  that references it. Reading the directory is the composition root's job, so
  `holon-frontend` never learns what a sidecar is.
- `crates/holon-frontend/src/preferences.rs` — `IntroducedSecret` and
  `introduced_secret_preferences` turn those into `Secret` `PreferenceDef`s.
  `PreferenceDef::env_override` changed from `Option<&'static str>` to
  `Option<String>`: a connection a user installed names its own variables, so
  that field is data now.
- `crates/holon-app/src/wiring.rs` — appends them to the schema at boot and runs
  `assert_secret_accounts_are_distinct` over the union, because a collision
  between a derived key and a declared one is only discoverable at runtime.

The label is humanised from the variable and the description names the
connection AND its file — the two compiled entries' hand-written text is kept
for the two compiled entries.

The load-bearing decision is the key spelling. The derived key is the variable
lowercased with underscores kept (`fixturebox_token`), NOT the dotted
`fixturebox.token` the bundled entries use. `normalize_var_name` folds `.` to
`_`, so for a hyphenated connection `my-own-thing` the dotted spelling would
normalize to `my-own-thing_token` and never match `MY_OWN_THING_TOKEN` — the
field would store a credential nothing reads. That case is a test of its own.

Covered by `crates/holon-app/tests/introduced_connection_secret_field.rs`. Red:
`lane-logs/item3-RED-1789179063.log`, produced by restoring the pre-fix
behaviour (the derivation returning nothing) — `Derived fields: []`. Restored
byte-for-byte, sha256 `12ddbb2f…` identical before and after
(`lane-logs/item3-teeth-before.txt`, `item3-teeth-after.txt`). Green:
`lane-logs/item3-first-1789178995.log`.

## Attribution

REGRESSION of `user-connections`. The compile-time list is older, but before
this lane no connection could exist that was not in it, so the gap was not
reachable. At `a5e161c0` the roster had no `Installed` arm.

## Second round — the fix opened a hole, and the verifier found it

The derivation above made a SECURITY defect reachable, and it is the more
serious half of what this entry now records.

`check_secret_namespace` asks whether a referenced variable starts with the
connection's own prefix. A connection named `todoist-api` owns `todoist_api_`,
and `${TODOIST_API_KEY}` starts with it — so the check PASSED and the file could
name the account holding the user's real Todoist token. The derived key
`todoist_api_key` then folded onto the bundled `todoist.api_key` account, and
the boot's distinctness assert aborted the app: a user file stopping the whole
application, which is exactly the defect
`2026-09-12-an-installed-connection-with-a-cleartext-url-panics-the-app-at-boot`
exists to close. Absent that abort, the file would have read the credential and
sent it to a host of its own choosing.

Fixed at ADMISSION, in `ConnectionRoster::scan`
(`crates/holon-mcp-client/src/roster.rs`): an introduced connection whose secret
namespace nests with any other provider's — in either direction — is rejected,
and the loader turns that into the same disclosed `IgnoredReason::Unusable`
every other refusal produces, so the app boots and names the file to rename.
Structural rather than per-reference on purpose: once two prefixes nest, no
variable name can say which provider a credential belongs to, so the ambiguity
is refused once instead of re-judged at every reference.

The second check I had added inside `introduced_secret_preferences` is DELETED.
With the boundary upstream the accounts are disjoint by construction, and a
second place for the rule to live is a second place for it to drift.

- Rule: `crates/holon-mcp-client/tests/introduced_connection_account_claims.rs`
  — the verifier's fixture, the mirror case (`claude` against the bundled
  `claude-history`), two introduced names that nest, and the case this rule does
  NOT reach (a bundled stem is an override, not an introduction). Red
  `lane-logs/sec-RED-1789185045.log` (3 of 5 failed), green
  `lane-logs/sec-GREEN-1789185179.log`.
- Delivery: `a_connection_claiming_a_bundled_secret_namespace_is_disclosed_rather_than_fatal`
  in `crates/holon-app/tests/invalid_connection_config_boots_degraded.rs`.

Credit where it belongs: this was found by the verifier on lane `uc-fixes`, not
by the lane, and not by any test the lane wrote.

## Third round — the same failure, by a route the second round did not close

The verifier found one more way to reach the boot abort, and it needs no second
provider at all. `mything.yaml` referencing BOTH `${MYTHING_TOKEN}` and
`${MYTHING.TOKEN}` is ONE connection with no nesting — but `secret_account`
folds `.` to `_`, so those are two names for one keychain entry. Two fields were
derived and `assert_secret_accounts_are_distinct` stopped the app.

The nesting rule from the second round could not see it, and the doc comment I
put where the deleted dedup had been — "the accounts are disjoint by
construction" — was false for exactly this shape.

Refused at admission beside the nesting rule, in `ConnectionRoster::scan`, which
already carries the file content. Refused rather than collapsed into a single
field: the two spellings do resolve to one credential, but a field declares only
ONE `env_override`, so an export of the other spelling would be in force while
the field still rendered editable — a user typing into a field whose value
nothing reads. The refusal names both spellings and the account they share.
Repeating the SAME spelling is ordinary authoring and is explicitly not a
collision.

The doc comment now states disjointness as a PRECONDITION on the caller, names
both rules that establish it, and records that the earlier "by construction"
claim was wrong — nothing in the type encodes it, and a reader who trusted the
type would repeat the mistake.

- Rule and cases: `crates/holon-mcp-client/tests/introduced_connection_account_claims.rs`
  — the fixture, the repeated-spelling non-case, and a combined-directory case
  holding all seven shapes at once (each earlier case uses its own directory,
  which cannot show the rules do not interfere). Red
  `lane-logs/r5-RED-1789190294.log`, green `r5-GREEN-1789190461.log`, combined
  `r5-combined-1789190904.log`.
- Delivery: `a_connection_spelling_one_account_two_ways_is_disclosed_rather_than_fatal`.

The lesson, since this is the second round of the same defect: after the first
fix I closed the shape I was shown and then asserted the general property. The
question that would have found this one is about `secret_account` — what else
folds two references onto one entry — not about provider names.

