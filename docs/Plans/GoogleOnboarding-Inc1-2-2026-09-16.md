# Google Onboarding: Increments 1 and 2 (plan)

Status: PLAN, amendments 1 to 10 of the senior review folded in. Base sentinel green (`grep -c holon-connections Cargo.toml` prints 2).
Scope: Google Calendar and Gmail, read plus write. Increment 3 (a Holon-owned OAuth client) is NOT here.
Lane: `.claude/worktrees/google-onboarding`, based on `integration` (`sw/w14-fix-slices`). Every citation below is verified against THIS lane's tree.
Governing design doc: `docs/adr/0034-low-code-connections-formats-and-systems-as-sidecars.md`. Proposal: `docs/Proposals/GoogleOnboarding-2026-09-16.md`. Policy research: `/tmp/holon-google-policy-research-2026-09-16.md`.

## 0. Five premise corrections, read this first

The proposal was written against an older reading of the tree. All five facts below change the shape of the work, and four of them make Increment 2w smaller.

1. **The `rest` transport is NOT GET-only.** `HttpMethod` carries Get, Post, Put, Patch and Delete (`crates/holon-mcp-client/src/rest_transport.rs:112-159`), a declared body template is placeholder-filled per call (`:386-392`), a body on a GET is refused at YAML load (`crates/holon-mcp-client/src/integration_config.rs:357-362`), and a 401 retry re-sends the body because the peer never saw the first attempt (`rest_transport.rs:525-565`). `assets/integrations/README.md:559-562` ("Non-GET methods fail loud today") is stale and must be corrected in the first increment that contradicts it.
2. **The write-policy machinery is built, not pending.** `writes:` per connection and `effect:` per tool (`crates/holon-mcp-client/src/mcp_sidecar.rs:490-497`, `:522-534`), deterministic intent keys for `keyed` and `once_only` (`crates/holon-mcp-client/src/mcp_provider.rs:691-724`), and a `once_only` at-most-once store with a manual-confirm path already wired into the app and the frontend approve panel (`crates/holon-mcp-client/src/write_authorization.rs:172-326`, `crates/holon-app/src/mcp_integrations.rs:523-532`).
3. **A generic, sidecar-parameterised write framework now exists.** `crates/holon-connections` holds a reconciler driven entirely by a sidecar's `holon.list_sync` block (`ListSyncSpec`, `spec.rs:21`), a `RemoteListPeer` transport trait (`sync.rs:186`), an `OperationProvider` (`RemoteListOperations`, `provider.rs:45`; `ConfiguredRemoteLists`, `registry.rs:82`), and a commit batch (`sync.rs:78-180`). `crates/holon-app/src/remote_list.rs` is its transport half, registered in DI at `crates/holon/src/di/registration.rs:330`. Section 2.2 explains why it does NOT fit Google Calendar.
4. **The real write gap is exactly one seam.** A `rest` integration registers `McpOperationProvider::read_only(...)` with an empty descriptor list (`crates/holon-mcp-client/src/mcp_integration.rs:1255`, `mcp_provider.rs:368-384`), and `execute_operation` hard-requires a `Peer<RoleClient>` (`mcp_provider.rs:632-645`). A rest sidecar has no peer.
5. **The credential intake already has a working precedent, and it is not the YAML keychain arm.** A `PrefType::Secret` preference configures a connector (`shopping.list_url`, `crates/holon-frontend/src/preferences.rs:353`), and the `${VAR}` resolver layers env, then keychain, then preferences by normalized name (`crates/holon-frontend/src/integration_vars.rs:37-56`). The proposal's YAML `client_secret_keychain` arm is the weaker route: `${VAR}` is what MARKS a value secret and strips it from logs, per ADR 0034 section 1. The proposal's "drag the JSON onto the row" mechanism also does not exist; see amendment 5.

Two things the proposal cites are absent from this line: `crates/holon-app/src/shopping_rest.rs` and a `ShoppingPeer`. The bespoke shopping client was replaced by `holon-connections`.

## 1. First principles

**Goals.** A person who owns a Google account, not a Google Cloud project, gets a working calendar and inbox. Reading works for any calendar provider. Writing works for Google Calendar and Gmail.

**Constraints.**
- ADR 0034 is binding: a format or a system attaches by authoring DATA, not Rust. Zero Rust per connection; one generic host per provider kind; one mapping language (jaq) shared by both halves.
- Secrets never in sidecars, only `${VAR}`. A `${VAR}` with no preference definition is refused at load (ADR 0034 section 6).
- An introduced connection may resolve only its OWN `${X_*}` namespace (`crates/holon-mcp-client/tests/introduced_secret_namespace.rs:1-15`).
- Platform-complete: macOS, Android and the `wasm32` web worker from one code path (ADR 0034 section 1). Any new dependency must build for all three.
- Parse, don't validate. Fail loud. Never swallow an error.
- Every behaviour change enters through a red-first PBT (`.claude/skills/holon-feature/SKILL.md`), then the dogfood-explorer gate.
- Standing rule (D118 note): a behaviour pinned only OUTSIDE the keystone is a reportable coverage gap.
- Google's scope tiers decide what is reachable: Calendar is not restricted, Gmail is (`gmail.readonly` restricted, `gmail.send` sensitive).

**Optimised for.** The smallest total change that removes the most user friction, in risk-elimination order.

## 2. Architecture

### 2.1 Current

One connector engine, plural transports, one neutral row contract. A sidecar carries a verbatim UTCP manual plus a `holon:` section (ADR 0034 section 5). Reads are transport-blind. Writes have THREE mechanisms, and choosing among them is the whole of Fork A.

| Mechanism | Shape it serves | Where |
|---|---|---|
| Sync-only replica | Poll a list or feed, replace-scope, no version cursor | the `sync` block; `rest_transport_mock.rs` Atom/RSS arms |
| Remote-list connection | Complete snapshot plus a version cursor, reconciled against local rows, batch commit | `holon.list_sync`; `holon-connections` |
| Operation descriptors, effect-gated | Individual calls, each classified by effect, with intent keys and an approval queue | MCP peers today; the `rest` path is refused |

Read leg, complete. `ResponseFormat` has arms Json, Atom, Rss (`rest_transport.rs:228-236`), decoded by `parse_feed` (`:886-915`) with `roxmltree`. The stated design rule: a feed "needs no new transport", only a new codec (`assets/integrations/README.md:489-553`).

Write leg, split in two. The transport half is done: method, body template, the `request` rows-to-arguments jaq mapper (`rest_transport.rs:87`, `:665-700`), `response_version_path` optimistic concurrency (`:442-470`), and a passing write suite (`crates/holon-mcp-client/tests/rest_transport_write.rs`). The reachability half is missing: descriptors are empty because MCP derives them from `peer.list_all_tools()` (`mcp_provider.rs:260-330`), and a rest manual has no peer.

Credential seam. Three arms per credential, exactly one allowed: `*_env`, `*_file`, `*_keychain` (`crates/holon-mcp-client/src/rest_oauth2.rs:65-113`, refusal at `:462-500`). Files are confined to the profile config dir and `client_secret_file` is 0600-enforced (`:130-155`). Both bundled Google sidecars point at the FILE arms (`assets/integrations/gcal.yaml:154-155`), which is the only reason the `jq` and `chmod` chore exists. The consent flow is built and pinned (`crates/holon-mcp-client/src/oauth_bootstrap.rs:777-851`); its missing-credentials text is the dead end the proposal identifies (`:802-809`).

Intake surfaces that exist. An introduced connection gets a derived secret Settings field (`crates/holon-app/src/introduced_secrets.rs:28`). A `PrefType::Secret` field is edited through a native platform dialog (`frontends/gpui/src/render/builders/pref_field.rs:309-404`) and written by `store_secret_preference` (`crates/holon-frontend/src/lib.rs:802-808`), keyed by `normalize_var_name(key)` under the one service `INTEGRATION_SECRET_SERVICE = "holon-integrations"` (`crates/holon-secrets/src/lib.rs:116`). There is NO file-drop target and no paste target anywhere: the only drag-and-drop is block reordering, with no external-paths handler. `platform_action` exists for `PrefType::DirectoryPath` and is the seam a file picker would extend.

Sidecar content resolution. An installed file enables its provider, but content comes from the compiled-in bundle unless the installed file declares `SIDECAR_SCHEMA_VERSION` (`crates/holon-mcp-client/src/bundled_sidecars.rs:1-19`).

Test tiers. The composed keystone does not cover integrations. The only integration-shaped keystone transition is `EmitMcpData` (`crates/holon-integration-tests/src/pbt/transitions/emit_mcp_data.rs`), a faithful no-op that no invariant observes. Integration behaviour is pinned by dedicated PBTs against local mock servers: `crates/holon-mcp-client/tests/rest_transport_mock.rs` (real sync path, Atom and RSS) and `oauth_bootstrap_flow.rs` (real `TcpListener` mock token endpoint, a recording browser, a keychain opener). Windowed behaviour lives in `crates/holon-integration-tests/tests/frontend_suite/`.

### 2.2 Fork A: how a Google write becomes reachable

- **A0. Ride the generic remote-list path** (`holon.list_sync` plus `holon-connections`). Cheapest on paper: a sidecar and no Rust. **Rejected for these providers.** The contract is a COMPLETE snapshot plus a monotonic version cursor plus a batch command endpoint (`ListSyncSpec`, `spec.rs:21-83`). Google Calendar's read is `events.list` over a TIME WINDOW, which is not the list, and its write is per-event `insert`/`patch`/`delete`, with no batch endpoint. A window in a complete-snapshot reconciler makes rows outside it read as absent, which under replace-scope semantics is the deletion hazard the spec's own comments warn about. Gmail has the same mismatch.
- **A1. Make the operation provider peer-optional.** Add a call-out seam to `McpOperationProvider` so the outbound call is either an MCP `Peer` or a `RestCallSurface`, and synthesize descriptors for a rest sidecar from what it already declares: the manual's `inputs:` is a JSON schema in the shape `input_schema_to_params` consumes (`assets/integrations/shopping.yaml:109-150`), and the top-level `tools:` map already carries `entity`, `effect`, `affected_fields`, `triggered_by` and `key_param` (`mcp_sidecar.rs:383-403`). The write-policy gate, intent keys and pending-write store are then reused unchanged across both transports.
- **A2. A separate `RestOperationProvider`.** Reimplements the `writes:` gate, the intent key and the `once_only` approval flow for rest only. Two copies of the ADR 0024 write contract, which will drift.
- **A3. A bespoke per-provider peer in Rust.** Contradicts ADR 0034's zero-Rust-per-connection principle.

**Decisive tradeoff.** A1 touches one shared type and keeps ONE write contract, which is the only way the effect gate stays trustworthy. A0 is refused by the data shape, not by cost. **Approved: A1.**

### 2.3 Fork B: where the client-JSON intake lives

- **B1. The preference schema plus `${VAR}`.** Define `google.client_id` and `google.client_secret` ONCE as `PrefType::Secret` (`crates/holon-frontend/src/preferences.rs:167-178`), and point BOTH sidecars at `client_id_env: GOOGLE_CLIENT_ID` and `client_secret_env: GOOGLE_CLIENT_SECRET` (amendment 4: one Google Cloud client serves Calendar and Gmail, so the user never pastes the JSON twice). Refresh tokens stay per provider. Reuses keychain routing, the distinct-account invariant (`:281-300`), the "Stored in the keychain" rendering (`frontends/gpui/src/render/builders/pref_field.rs:16-26`), the `${VAR}` redaction marking, and the load-time refusal of a `${VAR}` with no preference definition. Because `store_secret_preference` keys by normalized name under the single service `holon-integrations`, the credential lands under the SAME OS keychain service as every other integration secret.
- **B2. The YAML `client_secret_keychain` arm.** Works, but bypasses the preference definition and the `${VAR}` secret-marking, and files the credential under a sidecar-chosen service (`space.holon.gcal`, the commented example at `gcal.yaml:153`). Kept as a documented alternative only.
- **B3. Pass the JSON to `Configure…` as an operation parameter.** The credential then travels through an intent on every call rather than being stored once, and there is still no way to see that a credential is held.

**Decisive tradeoff.** B1 stores the credential once, in the keychain, behind the machinery whose tests assert a connector secret never reaches an error string. **Approved: B1.**

### 2.4 Fork C: the ICS decoder

Validated first, as required: Google's "secret address in iCal format" export carries VTIMEZONE blocks, `DTSTART;TZID=Europe/Berlin:...` and an RRULE for every recurring event, and so does nearly every real calendar. A decoder that refuses TZID and RRULE refuses almost every real feed, so C1 as originally written failed its own done-criterion. Corrected scope:

- **Tokenizer and component parser**, hand-rolled: line unfolding (CRLF plus a leading space or tab), property parameters (`NAME;PARAM=VALUE:VALUE`), and TEXT escaping (`\\`, `\;`, `\,`, `\n`).
- **Time resolution** needs a tz database: a `TZID` is resolved through `chrono-tz`. `TZID=Europe/Berlin` is an IANA name and resolves directly. A TZID that is NOT IANA (some exporters emit Windows zone names) is refused loudly by name, and the VTIMEZONE offsets are not used to synthesise a zone.
- **Recurrence** needs expansion: `rrule` expands an RRULE within a bounded window.
- **Refused loudly, never skipped**: `VTODO`, `VJOURNAL`, an unknown `VALUE` type, an unresolvable TZID. A single malformed VEVENT fails the whole feed naming the offending line, because a skipped record under replace-scope reads as a deletion.
- **Row identity**: `uid` for a VEVENT with no RRULE and no RECURRENCE-ID; `uid@<RFC3339 start>` for an expanded occurrence AND for an override, so a RECURRENCE-ID override naturally replaces the occurrence it overrides under replace-scope semantics.

**New dependencies, with their cost stated** (D107.a precedent: accepted when justified). `chrono-tz` and `rrule`, and nothing else; `rrule` pulls `chrono-tz` itself. Both are pure Rust with embedded tzdata. Verified in the tree: `check-web-arm` is a FEATURE set, not a wasm target, and `holon-mcp-client` is depended on only by holon-mcp-mock, holon-integration-tests, holon-app, frontends/mcp and frontends/gpui. It is NOT in the wasm worker's graph (`frontends/holon-worker` pulls holon-api, holon-core, holon-loro-wiring, holon-frontend) nor in `holon-frontend`, which is the crate with a `wasm32-unknown-unknown` check. So these crates' platform exposure is macOS and Android, not wasm, and `just gate-compile` covers the native build. Rejected: a full `icalendar` crate, which buys VTODO and VJOURNAL surface we do not ship and whose permissiveness works against parse-don't-validate.

### 2.5 Fork D: one feed or many

- **D1. One bundled `ics-calendar` sidecar** with `${ICS_CALENDAR_URL}` as a secret preference. Smallest change, mirrors `shopping.list_url`. One feed per install.
- **D2. The intake generates and writes a sidecar** under `{config_dir}/integrations/`, one per feed, relying on `introduced_secret_fields` for each credential. Gives many feeds, but the app gains a file-authorship capability and inherits the own-namespace rule (`${MYCAL_*}` only).
- **D3. The user hand-authors the sidecar**, which already works with no Rust. The advanced path.

**Decisive tradeoff.** D1 is small and safe and proves the codec and the intake seam; D2 is the real multi-feed answer but a bigger surface. **Approved: D1 for Increment 1, D2 named as the follow-up.** Reading "any calendar" is satisfied by D1, since any ICS feed works.

### 2.6 Recommended target

A1, B1, C1 as corrected, D1.

## 3. Increments

### Increment 1: the ICS rung (Google-free)

Delivers: one calendar by URL, read-only, no OAuth client, no consent screen, no Cloud console, no Google-specific code.

**Red first.**
- **Validate the fixture shape before the decoder (done, quick).** Add to `crates/holon-mcp-client/tests/rest_transport_mock.rs` an ICS fixture mirroring Google's documented export: VTIMEZONE, `TZID`, RRULE, EXDATE, RECURRENCE-ID, folded lines, escaped TEXT. The requirement is that this fixture DECODES, so the red is the missing `ics` codec, not a missing feature of real feeds.
- Sidecar-level red: a sidecar declaring `format: ics` loads. Today red at load, naming `ics` as an unknown response format. Confirm the red names the format and is not a build error.
- Row-level red: the fixture yields the expected occurrences, an EXDATE is absent, an override replaces its occurrence with the override's own start, and all-day events are distinguished.
- Refusal red: a `VTODO` and an unresolvable TZID each fail the feed naming the offending line. A skipped record is never acceptable.
- Settings red: a Settings write of `ics.calendar_url` configures the connector with no environment variable, is registered as a secret, and never reaches an error string (pattern: `crates/holon-app/tests/settings_shopping_list_url_credential.rs`).

**Implementation.**
1. Add `chrono-tz` and `rrule` to `crates/holon-mcp-client/Cargo.toml`.
2. Add `Ics` to `ResponseFormat` (`rest_transport.rs:228-236`), an `Ics` arm in `do_call`'s codec match (`:398-433`), and a decoder module beside `parse_feed` (`:886-915`).
3. **Sync window (amendment 2).** The feed is the full history, so expansion is bounded: 30 days back and 365 days forward by default, declared per tool in the sidecar and documented there. Rows outside the window are NOT replicated, and the sidecar says so in words, because an unbounded expansion of a 10-year calendar is not a feature.
4. Add `assets/integrations/ics-calendar.yaml`: a GET manual tool, `format: ics`, the window, an `event` entity with a `sync` block, `entity_prefix: "ics_"`, and the feed URL as `${ICS_CALENDAR_URL}`. Register it in `BUNDLED_SIDECARS` (`bundled_sidecars.rs:41-50`).
5. Add the `ics.calendar_url` secret preference in the Integrations section of `define_preferences` (`preferences.rs:301-304`). The key must normalize to the same keychain account the resolver reads.
6. Correct the stale GET-only paragraph in `assets/integrations/README.md:559-562` and document the `ics` codec beside the atom and rss ones.
7. **Record the D118 coverage gap (amendment 3).** The keystone reaches no configured connection for ICS either. File ONE bugfunnel entry under `docs/Testing/bugfunnel/entries/` naming both increments if the closing rung is the same, in the shape of `2026-09-14-remote-list-sync-keystone-unreachable.md`, and say what would close it: a transition plus an invariant (`wire()` in `composed_invariant_catalog()`, `crates/holon-integration-tests/src/pbt/composed/catalog.rs:348`).

**Gates.** `just keystone-smoke`; `cargo nextest run --features holon-integration-tests/pbt,holon-gpui/pbt -p holon-mcp-client --test rest_transport_mock`; the `holon-app` settings-credential test and its new sibling; `just lint`; `just gate-compile` (which typechecks the wasm arm, so the two new crates are proven platform-complete here). Then dogfood-explorer.

**Done when.** A pasted Google secret iCal address yields calendar rows including recurring occurrences, with an EXDATE honored and an override in place, and a non-Google ICS feed works identically. A malformed VEVENT or an unresolvable TZID fails loud with the offending line.

### Increment 2: credential intake (Google-free)

Delivers: the full OAuth path becomes terminal-free.

**Red first.**
- Schema-level: `define_preferences` declares `google.client_id` and `google.client_secret` as `PrefType::Secret`, and BOTH connectors resolve their client credentials through them. Today red: the keys are absent, so the sidecar file arms are the only source.
- Intake end state: after the intake runs on `{"installed": {...}}` and on `{"web": {...}}`, both connectors resolve the values; a JSON that is neither shape leaves the keychain untouched and surfaces a loud error.
- If a red turns out to be a build error rather than a refusal or a failed assertion, stop and report it. Never weaken a test to manufacture a red.

**Implementation.**
1. **Intake as a native FILE PICKER (amendment 5).** Because no drop target exists, add an operation `Import Google client JSON...` that opens a native file picker (extend `platform_action` to a file variant, the same osascript pattern as the existing directory picker), reads the chosen file, parses it at the BOUNDARY into an enum over the two shapes Google emits, refuses anything else loudly naming the file, writes the two preferences, then offers Configure. Paste into the existing secret field is the fallback on a platform without a picker. The JSON text itself is never stored, only the two derived values.
2. Switch both bundled Google sidecars to the shared `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` environment names (amendment 4).
3. Improve the consent flow's missing-credentials text (`oauth_bootstrap.rs:802-809`) to name the console URL and the exact fields, and to point at the import rather than a terminal.
4. Leave the refresh token on its file arm (`oauth_bootstrap.rs:633-638` says so deliberately).
5. Disclose the stale-file hazard: a previously hand-made `client_secret` file stays readable after the move. Offer removal.

**Gates.** `oauth_bootstrap_flow.rs` extended; the `holon-app` settings test and the new intake test; `just lint`; `just gate-compile`; `just keystone-smoke`. Then dogfood-explorer.

**Done when.** A user imports the downloaded client JSON through the picker, clicks Configure, consents, restarts, and has a working calendar and inbox. Both providers share one client identity, and the secret is never in a file the user manages.

### Increment 2w: write to Google Calendar and Gmail

Split into three landable rungs, riskiest first (amendment 9).

**2w-a: the A1 seam, provider-agnostic.**
- Start as a compile-only spike of the peer-optional provider, to de-risk the blast radius before any sidecar work. Then complete it fully, leaving no old path behind.
- Sidecar: a mock REST sidecar with ONE write tool. No Google.
- Red tests, both required: (i) a dispatch reaches the wire with the declared method and body, and reads back the declared `response_version_path`; today red with exactly "provider '...' is read-only: the `rest` integration exposes no write operations". (ii) a `writes: disabled` sidecar still refuses a non-read effect, proving the gate survived.
- Relax `reject_rest_out_of_scope` only where the new mechanism covers the case (`mcp_integration.rs:1186-1209`). A `rest` entity declaring `sync` still cannot declare `vtable.write_through`; that rejection is about two writers on one cache table and stays.
- Record the D118 coverage gap here (the bugfunnel entry from Increment 1 covers it if the closing rung is the same).

**2w-b: Google Calendar.**
- Write tools under `utcp:`, with `effect:` and `affected_fields:` in the top-level `tools:` map and `body` and `request` under `holon.tools`.
- Scope `calendar.readonly` to `calendar.events` (`gcal.yaml:164-165`): one consent covers read and write for events. `calendar.events` rather than `calendar`, because the wider scope grants calendar-list mutation we do not ship.
- **Scope change invalidates an existing consent (amendment 6).** An existing refresh token was consented for `calendar.readonly` only, so Google answers 403 `insufficientPermissions`. The row must disclose "re-run Configure", not report a generic sync failure. A red test pins that path.
- **Create is `keyed`, not `once_only` (amendment 7).** `events.insert` accepts a client-supplied `id` (base32hex, 5 to 1024 chars). Mint it from the Holon row key, declare `effect: keyed` with `key_param: id`, and creates become idempotent without queueing for approval. Validate that the field is accepted against Google's `events.insert` reference BEFORE implementing; if it is not accepted, fall back to `once_only` and say so in the sidecar. `update` and `delete` are `idempotent` or `keyed`.
- `writes: enabled` on the sidecar, or every non-read effect is denied loud.

**2w-c: Gmail send.**
- Scopes: `gmail.send` (sensitive) plus `gmail.readonly`. `gmail.modify` is RESTRICTED and stays out; label and archive are a separate decision for Martin. Send is `effect: once_only`, so it lands in the existing approval queue and never fires unattended.
- **Body encoding (amendment 8).** `messages.send` needs `raw` as base64url of an RFC 2822 message. Check whether `jaq-std` 3 exposes `@base64` and note that base64url is NOT standard jq. Plan an `encoding: base64url` directive on the `holon.tools` entry that applies to one mapped field, rather than a hand-written filter, with its own red test.

**Gates for each rung.** The `holon-mcp-client` write and mock suites, the new tests, `just lint`, `just gate-compile`, `just keystone-smoke`, then dogfood-explorer (2w-c's pass must include approve-a-pending-write).

**Non-Google calendar WRITE.** CalDAV is a listed follow-up. A1 makes a DECLARED write tool reachable, but CalDAV needs request shapes the manual cannot express today: a `PUT` carrying an iCalendar body, and a `REPORT` query. New transport surface, not a new sidecar.

## 4. Stays out of scope

- Increment 3: a Holon-owned OAuth client, and the ownership decision behind it.
- The YAML `client_secret_keychain` arm as the primary route (B2).
- A keychain arm for the refresh token.
- Multi-feed fan-out and app-authored sidecars (D2).
- Gmail label and archive (`gmail.modify`, restricted). Put to Martin.
- CalDAV read and write.
- Any change to the enablement model. The `.state.toml` stays the switch.
- The multi-calendar fan-out TODO already noted in `gcal.yaml:93-102`.

## 5. Risk register

| Risk | Kind | Assessment | Mitigation |
|---|---|---|---|
| A1 touches the shared provider every MCP connector uses | Correctness | Highest blast radius here | Compile-only spike first (2w-a); keep `read_only` as the no-write-tool arm; full `holon-mcp-client` suite plus `keystone-mcp` |
| The two new crates break a platform target | Build | `wasm32` and Android are both required; a break surfaces late if unchecked | `just gate-compile` typechecks the wasm arm, so it is caught in this increment |
| Recurrence expansion is unbounded or wrong at window edges | Correctness | A wrong or runaway expansion is worse than no row | Bounded window declared in the sidecar; fail loud past an occurrence cap; fixture the DST boundary and the window edges |
| An unresolvable TZID is common enough to refuse real feeds | Correctness | IANA TZIDs resolve; Windows-style names do not | Refuse loudly by name; revisit with a mapping only on evidence |
| A scope change silently breaks existing tokens | Usability | 403 after `calendar.readonly` to `calendar.events` | Disclose "re-run Configure" on the row; red test for that path |
| `events.insert` rejects a client-supplied `id`, so `keyed` is invalid | Design | Would force `once_only` and an approval queue for creates | Validate against the `events.insert` reference at rung start; fall back and say so |
| base64url has no jaq primitive | Design | `@base64` is standard jq; base64url is not | Declare `encoding: base64url` as a body directive with its own red test |
| The cookie-cutter risk: an installed sidecar shadows the new bundled content | Operational | The swap would appear to do nothing | Check at increment start that no installed `gcal.yaml` / `gmail.yaml` exists |
| A red is unreachable, for example a build error rather than a refusal | Process | The feature skill treats an impossible red as a reportable gap | Report and escalate; never weaken a test |
| The new behaviour is pinned only outside the keystone | Process | D118 note makes this reportable | Bugfunnel entry, written in Increment 1 |

## 6. Staleness guard

Re-run at the START of each increment.

```sh
# base sentinel (must print 2)
grep -c holon-connections Cargo.toml

# Inc 1
grep -n "enum ResponseFormat" -A8 crates/holon-mcp-client/src/rest_transport.rs
grep -rn "Ics" crates/holon-mcp-client/src/
grep -n "chrono-tz\|rrule" crates/holon-mcp-client/Cargo.toml
grep -n "ics.calendar_url" crates/holon-frontend/src/preferences.rs

# Inc 2 (shared Google client, one identity for both providers)
grep -n "client_id_file\|client_secret_file\|client_id_env\|client_secret_env" \
  assets/integrations/gcal.yaml assets/integrations/gmail.yaml
grep -n "google.client_id\|google.client_secret" crates/holon-frontend/src/preferences.rs

# Inc 2w
grep -n "McpOperationProvider::read_only" crates/holon-mcp-client/src/mcp_integration.rs
grep -n "fn reject_rest_out_of_scope" -A8 crates/holon-mcp-client/src/mcp_integration.rs
grep -n "pub enum HttpMethod" -A10 crates/holon-mcp-client/src/rest_transport.rs
grep -n "pub request: Option<Arc<RowMapper>>" crates/holon-mcp-client/src/rest_transport.rs
grep -n "pub struct ListSyncSpec" -A12 crates/holon-connections/src/spec.rs

# docs drift
grep -n "GET only\|Non-GET methods fail loud" assets/integrations/README.md

# sidecar resolution
grep -n "SIDECAR_SCHEMA_VERSION" crates/holon-mcp-client/src/bundled_sidecars.rs
```
