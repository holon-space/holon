# S2 — Kind fidelity on the Loro leg (+ S3, the derived sidecar)

**Goal.** A `Value::DateTime` / `Value::Json` written into the Loro document
reads back as the same variant after a restart. Today it reads back as
`Value::String`.

**Scope owner.** `docs/Plans/BlockGeneralization.md:70-84` (NV-1/S2–S3 row).
NV-1 (`e19a726f`) bought this on the SQL leg only and *disclosed* the Loro gap
in the profile comment at `assets/default/capability/holon-native.yaml:176-180`.

---

## 1. Premise table — where untagged bites

`Value` is `#[serde(untagged)]` at `crates/holon-pattern/src/value.rs:54`.
Untagged serialization emits the *payload*, so `DateTime(String)` and
`Json(String)` both go out as a bare JSON string, and untagged deserialization
takes the first matching variant — `String` (`value.rs:65`) — before ever
reaching `DateTime` (`:72`) or `Json` (`:76`).

### 1a. Kind survival across a Loro write→read round trip

| Kind | On-disk JSON | Reads back as | Verdict |
|---|---|---|---|
| `DateTime("2026-08-22T10:00:00Z")` | `"2026-08-22T10:00:00Z"` | `String` | **LOST** |
| `Json("{\"a\":1}")` | `"{\"a\":1}"` | `String` | **LOST** — byte-identical to `String` of the same text |
| `Removed` | `{"__holon_removed":true}` | `Removed` | safe — NV-2 gave it a shape only it matches (`value.rs:14-44`) |
| `Boolean` / `Integer` / `Float` / `Null` / `Array` / `Object` | distinct JSON shapes | same | safe at top level |
| `DateTime`/`Json` **nested** inside `Array`/`Object` | bare string | `String` | **LOST** — see §4 |

`AmbiguousKind` (`crates/holon-pattern/src/property_kinds.rs:22-28`) already
names exactly this set: `DateTime`, `Json`. `AmbiguousKind::of(&Value)`
(`:36-42`) is the shared law — it returns `Some` for precisely the values whose
JSON form does not name their kind.

### 1b. Every `Value` ↔ Loro crossing

The boundary is **narrow**: one encode function, two decode functions, all in
`crates/holon-loro/src/loro_backend.rs`. The wire form is one JSON string per
property key inside the nested `PROPERTIES_MAP` `LoroMap` (`:738`).

| Leg | Site | Note |
|---|---|---|
| WRITE | `loro_backend.rs:778` `encode_property_value` | **the single choke point** — `serde_json::to_string(value)` after `reject_reserved_marker` |
| WRITE (callers) | `:849` merge · `:874` replace · `:895` per-field · `:828` legacy-blob migrate | all route through `encode_property_value` |
| READ | `:795` `decode_properties_map` | `serde_json::from_str::<Value>` per key |
| READ | `:749` `read_scalar_field_from_meta` | per-key cell read; must agree with the above |
| READ (legacy) | `:412-425` `read_properties_from_meta` blob fallback · `:769` | pre-H3 single-blob `PROPERTIES`, self-heals on next write |
| Cell | `crates/holon-loro/src/loro_meta_cell_backing.rs:42-97`, `:209` | `LoroScalarField::encode/decode` over `Value` |
| S3 sidecar | `crates/holon-turso/src/derived_reconciler.rs:149` | `serde_json::to_string(&value)` stored as bare `Value::String` into `block_derived.value_json` (`:161-171`); no kind map beside it |

**Not affected** (these never carry a `Value`): marks (`:230`, `:86`), edge
fields `tags`/`requires` (`:580`, `:636`, `:1157` — `Vec<String>`), source
`header_args` (`:728`, `:380`), the sharing leg (`loro_share_backend.rs:1327`).

### 1c. The blast radius is smaller than it looks

The Loro→Turso projection (`loro_sync_controller.rs:1706`, `:1838`) passes
**typed `Value`s** through as params, and `SqlOperationProvider` computes the
kinds itself — `PropertyKinds::of(...)` at
`crates/holon/src/core/sql_operation_provider.rs:1373` and `:1582`. So the SQL
leg is faithful *given a faithful `Value`*. **Fixing the Loro read choke point
makes the projection faithful for free** — no new plumbing on that path, only a
restart-then-project test to pin it (Inc 2).

### 1d. Format versioning today

| Question | Answer |
|---|---|
| Is there a schema version cell? | Yes, **but it is inert**: `loro_backend.rs:2023` writes `_meta._schema_version = 2`. Repo-wide grep finds **no reader**. |
| Is there a migration framework? | **No.** Format evolution is handled by *self-healing dual-form reads*: the pre-H3 blob fallback at `:412-425`, `:769`, `:828`. This is the established precedent. |
| On-disk artifact | `.loro` binary snapshots. `holon_tree.loro` (`loro_document_store.rs:50-51`, path `:81`, save `:196`), atomic tmp+rename `loro_document.rs:274/330`, load `:297-313`, dir `<orgmode_root>/.loro` (`holon-app/src/wiring.rs:181`). Shares: `shares/<id>.loro`. |

**Martin's vault holds real `.loro` documents.** Silent re-interpretation is
forbidden, so the migration story is the deciding axis in §2.

---

## 2. Design options — first principles

The fundamental question: **where does the kind live relative to the value?**

| | **A′ — tag the ambiguous value in-band** *(recommended)* | **B — adjacent kinds map** (mirror NV-1) | **C — versioned envelope + rewrite** |
|---|---|---|---|
| Form | `AmbiguousKind::of` returns `Some` → store `{"__holon_kind":"date_time","v":<payload>}`; otherwise store exactly today's bytes | second `LoroMap` `_property_kinds`, key → kind | bump `_schema_version` 2→3, rewrite every property on load |
| **Migration of existing `.loro`** | **none needed.** Unambiguous values are already byte-identical. Reader accepts both forms; a write self-heals — the exact pattern already at `:412`/`:828` | none needed (values untouched) | **full rewrite of the vault on first load** |
| CRDT correctness | **one fact, one register.** Kind and value merge together, always | **two registers for one fact.** Peer A sets `when`=DateTime, peer B sets `when`=String; Loro LWW-merges the two maps *independently* → B's string can land under A's `date_time` kind. NV-1 is safe on SQL only because bag+kinds share ONE atomic UPDATE (`sql_operation_provider.rs:1381`); Loro offers no such atomicity across containers | one register |
| Nested kinds (§1a row 5) | fixed for free (tagging is recursive) | **not fixed** — keyed by property key only | fixed |
| Downgrade (older build reads new file) | previously-broken keys read as an `Object` carrying the payload — visibly odd, **no data destroyed**, and those keys were already broken before S2 | reads as today (String) — unchanged | **reads envelopes as plain `Object`s vault-wide — silent re-interpretation. Forbidden.** |
| Mixed-version pair (old build **writes**) | old build never tags, so a new build reads its `DateTime` as `String` — see §2a | same | envelope absent → same |
| Shared vocabulary with NV-1 | reuses `AmbiguousKind` verbatim | reuses `PropertyKinds` verbatim | new |
| One-way door? | no | no | **yes** |

**Recommendation: A′.** The deciding tradeoff is B's split-register merge
hazard: it re-creates, across CRDT peers, precisely the bag/kinds-disagreement
failure NV-1's single-writer rule was invented to prevent — and on Loro it
cannot be closed by a single-writer rule, because concurrent peers are the point
of the leg. A′ keeps kind and value in one register, needs no migration of
existing bytes, and fixes nesting for free. C is rejected on the migration axis
alone.

**Collision hazard and its closure.** An authored `Value::Object` whose shape
matches the envelope would be misread. This is the *same* hazard
`REMOVED_MARKER_KEY` already has, and it already has machinery: extend
`Value::reject_reserved_marker` (`value.rs:282-294`) — which walks to any depth
(`reserved_marker_path`) — to also reject the kind envelope. A colliding value
then fails loudly at the write, where the author is present, and can never reach
storage. Use a key in the same reserved namespace (`__holon_kind`).

**Decode is exact-shape-or-loud.** A stored map containing `__holon_kind` that
is not *exactly* `{__holon_kind: <known kind>, v: <payload>}` — unknown kind
spelling, missing `v`, extra keys — is a **loud parse error**, never a
fall-through to a plain `Object`. Falling through would silently accept a
malformed envelope; since the write-side rejection above makes authoring one
impossible, anything malformed in storage is corruption and must say so. This
matches the leg's existing posture, where a non-string or unparseable property
already panics (`loro_backend.rs:795-812`). Pinned by a unit test.

### 2a′. The limit of "no migration" (disclosure)

"No migration needed" is forward-looking, and there is one pre-existing case it
does not cover. A vault written BEFORE S2 that already held a literal property
object shaped like `{"__holon_kind": …}` is now read differently: a well-formed
one is silently re-interpreted as the kind it names, and a malformed one is
refused loudly at the read. Nothing detects or migrates those.

Reachability is near zero — `__holon_kind` is a key this change invented, so no
prior writer emitted it — and the exposure is identical in kind to the
`__holon_removed` precedent NV-2 accepted on the same leg for the same reason.
Stated here rather than left implicit, because the claim above is otherwise
unqualified.

### 2a. Mixed-version pairs (disclosure)

During any window where an **old build writes** and a new build reads, the old
build never tags, so its `DateTime`/`Json` still arrive as `String`. This is the
**pre-existing** loss, not a regression S2 introduces, and it is acceptable —
but it must be stated rather than assumed away, because the sharing leg makes
mixed versions a *real* state rather than a hypothetical: shared documents pair
peers on different release cadences (Mac↔Android), so "every writer is current"
is not an invariant the design may lean on. S2 makes a *new* writer faithful; it
cannot retroactively make an *old* writer faithful. The same sentence belongs in
the profile disclosure comment at `holon-native.yaml:176-180`.

---

## 3. Increments — risk-elimination first

Each is independently landable. Each names its red-first surface.

| # | Increment | Risk it eliminates | Red-first surface |
|---|---|---|---|
| **S2.0** | Add a Loro `Carrier` to `carriers()` at `crates/holon/tests/capability_certification.rs:414-416` (today only `BLOB_LEG`, `:43`), so the certifier drives `round_trip_property` over the Loro leg. Land it with a test that **asserts the two violations exist** (`Violation{ clause: TypeDeclared{DateTime\|Json}, route:"create", leg: loro }`, `certify.rs:976-985`) | "Is the loss real, and does the certifier see it?" — measures before changing anything | The asserted-violation test **is** the red log. Landable because the pin asserts the loss rather than denying it |
| **S2.1** | Tag ambiguous kinds in-band. Encode: `loro_backend.rs:778`. Decode (dual-form, envelope first then today's parse; **exact-shape-or-loud** per §2) : `:795`, `:749`, plus legacy-blob paths `:412-425`, `:769`, `:828`. Guard: extend `reject_reserved_marker` (`value.rs:282`). **The declarations move WITH the fix** — the Loro-leg disclosure comment at `holon-native.yaml:176-180` and the Loro `Carrier` description land updated in *this* increment, because a declared loss the certifier can prove is gone is itself a false declaration (NV-1's rule) | The loss itself | **Flip S2.0's pin to `report.is_clean()`** — the NV-1 flip pattern (`capability_certification.rs:527`). Plus a written-then-reloaded-from-disk round trip over `holon_tree.loro`, and a malformed-envelope unit test asserting the loud error |
| **S2.2** | Pin the through-line: write DateTime/Json → save snapshot → reload → project to Turso → read back. Per §1c this should pass with **no production change**; if it does not, the surprise is the deliverable | "Does S2.1 actually reach the user, or does the projection re-flatten?" | A restart-then-project test that is red before S2.1 and green after |
| **S2.3** | Nested kinds inside `Array`/`Object` (§1a row 5) — free if S2.1's tagging is recursive, a follow-up if not | Silent loss one level down | Certifier specimen with a `DateTime` inside an `Array` |
| **S3** | Derived sidecar: `block_derived` gains a kind column written from `AmbiguousKind::of(&value)` at `derived_reconciler.rs:149`, parsed at the one read boundary. Independent of S2 | Same loss shape as pre-NV-1 `block_raw.properties` | Certifier route over the derived sidecar, or a targeted red test at the reconciler |

Ordering rationale: S2.0 buys measurement before change; S2.1 is the whole fix;
S2.2 is verification that may cost nothing; S2.3 and S3 are separable.

---

## 3a. What landed (2026-09-01) and the one surprise

S2.0/S2.1/S2.2 are done; S2.3 and S3 stay open.

| Claim | Evidence |
|---|---|
| Ambiguous kinds survive a RESTART on the Loro leg | `date_time_and_json_keep_their_kind_across_a_restart` — saves a real `.loro` snapshot, reopens from bytes, reads through the production projection. Red before: `left: Some(String("2026-08-22T10:00:00Z"))` |
| The profile's `types` claim holds on the Loro leg, every route | `the_loro_leg_keeps_every_declared_kind` drives `create` + `update_block_properties`, reading the kind list FROM the yaml. Red before: `Present(String(…))` vs `Present(DateTime(…))` |
| A look-alike cannot be authored | `an_authored_kind_envelope_look_alike_is_refused` (nested, names the key path) |
| A malformed envelope is loud | `kind_envelope::tests::a_malformed_envelope_is_loud` — 6 shapes, each refused |
| Existing documents need no migration | Unambiguous values take the untouched `serde_json::to_string(value)` branch; only ambiguous kinds change bytes |

**S2.2 answer: §1c HELD — the through-line cost nothing, and it is now
MEASURED.** No production change was needed beyond S2.1:
`loro_sync_controller.rs:1705` passes the typed `Value` straight through as a
param, and `SqlOperationProvider` re-derives kinds from it
(`sql_operation_provider.rs:1373`, `:1582`).

Pinned permanently by
`crates/holon-integration-tests/tests/loro_suite/loro_kind_fidelity_through_projection.rs`
— prod session → Loro CRUD authority → projection → SQL read boundary, asserting
both halves: `block_raw.property_kinds` equals `{"when": DateTime, "doc": Json}`
(compared as a parsed `PropertyKinds`, not as text) AND the read boundary hands
those keys back typed, with a byte-identical authored `String` staying a
`String`. Disabling the encode branch turns it red at the FIRST half —
`left: PropertyKinds({})` — which is what proves the assertion runs through the
Loro leg rather than restating NV-1.

**The surprise — an authoring-boundary clause cannot be certified per-leg.**
Adding `LORO_LEG` to `carriers()` produced 4 violations that have nothing to do
with kinds: `property_keys.engine_owned_keys` for `_provenance` and
`property_kinds`, on both Loro routes. `certify` runs EVERY clause over every
carrier, and that clause asks whether *authoring* the key is refused. The
refusal lives at the operation engine (`operation_engine.rs:400`), which then
STAMPS `_provenance` itself (`:423-426`) — so **every storage leg must accept
the key it is being asked to refuse**. `BLOB_LEG` passes only because its probe
goes through `execute_op`; a storage-leg probe structurally cannot.

Resolved for now by driving the Loro leg through a dedicated test instead of a
second `Carrier`, which keeps the measurement real and the suite honest. The
open question for the certifier's model is in §6.

## 4. Explicitly out of scope

- **Making `Value` tagged globally.** The `#[serde(untagged)]` is load-bearing
  for flutter_rust_bridge interop (`value.rs:46-52`, `:69-76`). S2 changes the
  *Loro storage form*, not the FRB wire form.
- **NV-1's nesting gap on the SQL leg.** `PropertyKinds` is keyed by property
  key only (`property_kinds.rs:57`), so nested kinds are lost there too. S2.3
  fixes the Loro leg; the SQL leg stays as NV-1 left it.
- **Activating `_schema_version`.** Building a real Loro migration framework
  (`loro_backend.rs:2023` is write-only) is Inc 4 territory, not S2.
- **Inc 4, the Loro format adapter** (`BlockGeneralization.md:118-123`) — it is
  *blocked by* S2, not part of it.
- **Marks, edge fields, `header_args`, the sharing leg** — no `Value` crosses.
- **Float `NaN`/`Inf`**, which already fail loudly at `serde_json::to_string`.

## 5. Staleness guard — re-run at the start of every increment

```
cd <lane>
grep -n 'serde(untagged)' crates/holon-pattern/src/value.rs                  # expect :54
grep -n 'fn encode_property_value\|fn decode_properties_map\
\|fn read_scalar_field_from_meta' crates/holon-loro/src/loro_backend.rs      # expect 778 / 795 / 749
grep -rn 'property_kinds\|AmbiguousKind' crates/holon-loro/                  # expect ZERO — non-zero means someone chose option B
grep -n '_schema_version' -r crates/                                         # expect ONE hit (write-only); a reader means a migration framework arrived
grep -n 'BLOB_LEG\|fn carriers' crates/holon/tests/capability_certification.rs
grep -n 'types:' assets/default/capability/holon-native.yaml                 # expect :207 still claiming date_time, json
grep -n 'PropertyKinds::of' crates/holon/src/core/sql_operation_provider.rs # expect 1373 / 1582 — §1c rests on these
```

If the `holon-native.yaml:176-180` Loro disclosure comment is gone, S2 has
already been attempted — read the history before proceeding.

## 6. Open question for Martin (certifier model, not S2)

**Should `property_keys.engine_owned_keys` be a per-carrier clause at all?**
It is declared under `property_keys` and driven once per carrier, but it asks
about the AUTHORING boundary, and the engine stamps `_provenance` into the
params every storage leg then stores. So no storage leg can refuse it, and any
carrier whose probe does not pass through `execute_op` reports a violation that
is about probe altitude, not about the leg.

- **(a) Drive it once, at the boundary that owns it** — move the clause out of
  the per-carrier loop in `holon-capability/src/certify.rs`, or mark it
  DEFERRED-to-Operation-layer the way 10 other clauses already are. Then a leg
  can join `carriers()` without inheriting a clause it cannot answer.
- **(b) Keep it per-carrier** — then every carrier's probe must enter through
  the authoring boundary, which means the Loro carrier needs a Loro-backed
  operation engine in the harness (real work, and it would also give S2.2 its
  missing end-to-end pin).

Recommendation: **(a)**, because the profile comment itself already names
`reject_engine_owned_keys` in `operation_engine.rs` as the enforcer — the
clause is documented as an engine guarantee, so certifying it per storage leg
restates the wrong thing. (b) is strictly more work for a weaker claim.
Blocking nothing today; it blocks adding a second `Carrier` later (Inc 4).
