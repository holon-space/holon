# Verify `lowcode-inc1` — **REFUTED**

WS `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/lowcode-inc1`, `@ = 30bb16a3` on `main 89e2efea`.
All gates re-run by me. Sources restored and sha256-verified after every mutation.
Logs: `lane-logs/verify-g1-1.log`, `verify-teeth1.log`, `verify-teeth2.log`, `verify-g2.log`, `verify-g3.log`, `verify-g4.log`, `verify-g5.log`, `verify-g6.log`.

## Refutations

### R1 (P1, silent data corruption) — `Value::Json("null")` becomes `Value::Null`

```
WIRE:
{"holon_rows":1,"scopes":[{"type":"a","owner_column":"owner","owner_value":"vault/file","kinds":{"j":"json"}}]}
{"type":"a","row":{"j":null}}
  left:  [ ... rows: [{"j": Null}] ]          <- parsed back
  right: [ ... rows: [{"j": Json("null")}] ]  <- emitted
```

`emit.rs` accepts it: `serde_json::from_str("null")` is `Ok`, so the "unparseable Json" guard does not fire, and `From<Value> for serde_json::Value` (`crates/holon-pattern/src/value.rs:596`) turns it into JSON `null`.
`parse.rs:restore()` returns `Ok(Value::Null)` on `json.is_null()` **before** consulting `kinds` — the envelope says `json`, and the parser ignores it.

This breaks the crate's own rule 2 (NULL distinct, never fabricated) and rule 4 ("a NULL nobody wrote"), on a shape a jaq filter or a remote JSON API produces routinely. Emit must refuse it, or the kind must win over `is_null()`.

**Generator blind spot that let it through:** `contract_pbt.rs:75` — `Kind::Json` only ever produces `\{"k":(0|[1-9][0-9]{0,2})\}`. No `null`, no scalar document. (Scalar docs `false`/`0`/`"s"`/`[]`/`1.5` do round-trip — I checked; only `null` breaks.)

### R2 — a duplicated column is silently last-wins

`{"type":"a","row":{"x":1,"x":2}}` parses to `Ok([... rows: [{"x": Integer(2)}]])`. `serde_json::Map` overwrites. Rule 3 says a malformed line is an `Err`; this one is accepted with a value the producer did not unambiguously state.

### R3 — the report's §6(a) blast-radius claim is false in the direction that matters

`float_roundtrip` is **not** workspace-wide. `holon-rows` is a **dev-dependency** of `holon-kitchen` only (`crates/holon-kitchen/Cargo.toml:29`), and nothing else depends on it, so:

```
cargo tree -p holon-gpui -e features -i serde_json | rg -c float_roundtrip  -> FLOAT_ROUNDTRIP-ABSENT-FOR-GPUI
cargo tree -p holon      -e features -i serde_json | rg -c float_roundtrip  -> FLOAT_ROUNDTRIP-ABSENT-FOR-HOLON
```

`workspace-hack/Cargo.toml` does not carry the feature either. The shipped app keeps the 1-ULP drift; only test builds that pull `holon-rows` get the fix. The workspace-wide-change concern raised for Martin's ruling is currently moot, and the fix is not where it was claimed to be.

## Confirmed

- `crates/holon-rows/Cargo.toml` present; workspace member (`Cargo.toml:21`) + workspace dep (`:266`).
- **75/75** `-p holon-rows -p holon-kitchen` COLD (no proptest-regressions existed) and at `PROPTEST_CASES=512`; **155/155** `-p holon-core -p holon-architecture-tests`. `cargo fmt --check` clean.
- **Teeth 1** (`kinds: kinds_of(..)` -> empty map): `75 tests run: 71 passed, 4 failed` — `a_datetime_column_does_not_decay_into_a_string`, `a_json_column_does_not_decay_into_a_string`, `a_json_column_comes_back_canonically_spelled`, `a_row_stream_round_trips`. Exactly the lane's claimed red.
- **Teeth 2** (parser fills absent columns with `Null`): `75 tests run: 74 passed, 1 failed` — only `a_row_stream_round_trips`. The NULL-vs-absent distinction has teeth, but **only** through the generator PBT; `a_null_column_stays_null_and_never_becomes_a_zero` stayed green, so there is no named example test for it.
- wasm: `-p holon-rows --target wasm32-wasip1-threads` OK; `-p holon-rows -p holon-frontend --target wasm32-unknown-unknown` OK. `-p holon-rows` **alone** on `wasm32-unknown-unknown` fails on `getrandom`/`uuid` — **pre-existing**, not this lane: `cargo check -p holon-core --target wasm32-unknown-unknown` fails identically.
- `cargo check -p holon-gpui` OK.
- No dual serialization path: `TypedRowSet` appears outside `holon-rows` only in `crates/holon-kitchen/src/rows.rs`, `crates/holon-core/src/{file_format,lib}.rs`, `crates/holon/src/core/typed_row_sink.rs`; none of them calls `serde_json::to_*` on it.
- `float_roundtrip` blast radius (test builds): `-p holon-turso -p holon-loro -p holon-app --lib` -> `553 tests run: 553 passed, 3 skipped`. No float-format regression.
- Adversarial shapes that PASS: empty set list, zero-row scope, undeclared type (`Err`, names `line 2`), bad value for a declared kind (`Err`, names line **and** column), DateTime at the epoch / `+00:00` / `+05:30` / nanos with `-08:00`, `-0.0` keeps its sign, integral float stays a float, 1 MB unicode string, unicode + `type`/`row`/empty/newline column names, NULL vs absent distinct on the wire and back, trailing blank line refused, `holon_rows: 2` refused, NaN/Inf/`{`-as-Json refused, an RFC3339-looking `String` stays a `String`.

## Non-blocking

- `cargo clippy -p holon-rows --all-targets -- -D warnings` fails, but in **`holon-api`** (54 `double_must_use` errors) — untouched by this diff, pre-existing.
- `cargo hakari verify` is non-zero (`workspace-hack` not regenerated for the new member). I found no hakari gate in `justfile`/`.github`.
- The teeth runs wrote `crates/holon-rows/tests/contract_pbt.proptest-regressions`; I deleted it. My scratch test file is removed from the tree.

---

# Rev 2 — **CONFIRMED**

All three refutations are fixed. Re-run in the same WS; sources and `Cargo.toml`
restored sha256-verified after the teeth. Logs: `lane-logs/verify-rev2-a.log`,
`lane-logs/verify-rev2-teeth3.log`.

## R1 — fixed

`parse.rs:101-107` now lets the kind decide **before** nullness (`Some(AmbiguousKind::Json)`
arm precedes `_ if json.is_null()`), and `emit.rs:144-160` refuses the ambiguity the
old code silently resolved.

- `Value::Json("null")` round-trips exactly.
- `false`, `0`, `"s"`, `[]`, `1.5`, `null`, `{"k":1}`, `[null,1]` as `Json` documents all round-trip.
- A `Json` document and a NULL in one scope refuse at emit, naming **both** type and column:
  `evt.payload holds a JSON document in one row and a NULL in another, and both reach the wire as \`null\``.
  Refused in either row order (document-then-null and null-then-document).
- `DateTime` + NULL in one scope is still legal and round-trips — the new guard is scoped to `Json`, correctly.

## R2 — fixed

`envelope.rs:59-98` replaces `serde_json::Map` with an order-preserving `RowCells`
whose visitor rejects a repeated column.

- `{"x":1,"x":2}` -> `Err`: `line 2 is not a row: column "x" is stated twice, at position 1 and position 2`.
- `{"x":1,"y":2,"x":1}` (identical duplicate) -> `Err`, positions 1 and 3.
- A duplicated field in the **envelope** is still refused.
- The row is still a JSON object on the wire (`collect_map`), so the jaq contract is intact.

## R3 — fixed

`float_roundtrip` moved to the root `[workspace.dependencies]` (`Cargo.toml:69`), so
every member gets it rather than only test builds pulling `holon-rows`.

- `cargo tree -p holon-gpui -e features -i serde_json` now lists `float_roundtrip`
  with `holon`, `holon-advice`, `holon-api`, … under it.
- Teeth: with the feature removed in a cp-aside `Cargo.toml`, **both** new float
  tests go RED for the right reason — `holon-rows::contract_pbt a_float_comes_back_to_the_last_bit`
  and `holon-loro …::a_stored_float_property_comes_back_to_the_last_bit`, each
  `got -57093562.38074909`. The loro test proves the feature reaches a production leg.
- **Correction to the request:** `jj diff Cargo.lock` is **not** empty (15 lines) and
  cannot be — the new workspace member and `holon-kitchen`'s dev-dep edge must be
  recorded. The delta is exactly the `holon-rows` package entry plus that one edge;
  no version or resolution churn, and features do not appear in the lock at all.

## Gates

- `PROPTEST_CASES=512 cargo nextest run -p holon-rows -p holon-kitchen --test-threads 4`, COLD
  (regressions files deleted first): `Summary [0.973s] 91 tests run: 91 passed, 0 skipped`
  — 81 lane tests plus my 10 probes.
- `cargo check -p holon-gpui` OK.
- Pre-existing, unchanged from Rev 1: `cargo clippy -D warnings` red inside untouched
  `holon-api`; `cargo hakari verify` non-zero (no CI gate found for it).
