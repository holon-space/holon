---
id: 2026-09-19-live-block-id-that-forms-no-uri-kills-the-window
date: 2026-09-19
gap: COVERAGE
secondary: PERCEPTION
status: FIXED
summary: >-
  A `live_block("my task")` in author-written render DSL unwound inside
  `EntityUri::parse(..).expect` and took the whole window down; two further
  `live_block` legs re-schemed an already-failed parse through the panicking
  `EntityUri::block`, and three keyed row providers defaulted an id-less row
  to `EntityUri::block("")`, which parses and so aliased distinct rows onto
  one phantom identity.
---

## Bug

Found by CODE AUDIT, not by an automated test: the `block-ctor-sweep` lane
swept every production `EntityUri::block(` / `from_raw(` / `new("block"` site
whose argument is not a compile-time literal, after a verifier reading the
D142.a follow-up flagged the constructor as panicking. A second verifier pass
on that lane's own diff (`lane-logs/block-ctor-sweep-verify.md`) found the
remaining consumers recorded below. No user reported it and no suite drew it.

`EntityUri::block(s)` is `EntityUri::new("block", s)`, and `new` panics when
`block:{s}` forms no URI (`crates/holon-api/src/entity_uri.rs:134`). Four
production shapes handed it exactly the input already known to be bad, and one
handed it a default that does NOT panic and is therefore worse.

Reachable from user input today — the render DSL leg. A `:RENDER:` drawer or a
`block_profile` render expression is author-written text, so
`live_block("my task")` reached
`EntityUri::parse(&s).expect("live_block: invalid entity URI")` at
`crates/holon-frontend/src/shadow_builders/live_block.rs:11` and unwound:

```
thread '…' panicked at crates/holon-frontend/src/shadow_builders/live_block.rs:11:43:
live_block: invalid entity URI: Invalid URI "my task": unexpected character at index 2
```

Latent — the remaining legs. `frontends/gpui/src/render/builders/live_block.rs:19`
and `frontends/gpui/src/views/reactive_shell.rs:1085` both read the `block_id`
prop and, on a failed parse, re-schemed the SAME value through
`EntityUri::block`:

```
thread '…' panicked at crates/holon-api/src/entity_uri.rs:134:13:
EntityUri::new("block", "my task") produced invalid URI: unexpected character at index 8
```

They are latent only because `ViewModel::live_block` is the prop's sole
producer and takes a typed `EntityUri` — the shadow builder's panic fires
first. Fixing the reachable leg without these would have made them reachable.

Silent, not loud — the three keyed row providers. `focus_chain.rs:104`,
`chain_ops.rs:133` and `value_fns/synthetic.rs:50` each keyed a row with
`data_row_entity_uri(&row).unwrap_or_else(|| EntityUri::block(""))`. `block:`
IS a valid RFC 3986 URI, so this never panicked: every id-less row took the
same key, and the keyed `SignalVec` aliased rows that are not the same row.
This is disposition 4 of the repo's error-handling order — silently degrading
to look fine.

## Root cause

`EntityUri` offers a panicking constructor (`block`) and a fallible one
(`try_from_raw` / `parse`) with no type-level distinction at the call site, so
"parse failed, now what" was answered locally at each site by the constructor
that compiles without a `Result`. The D142.a boundary ruling — ids are parsed
ONCE at the boundary, a failed parse is an error surface — had no enforcement
for this particular shape: `archlint`'s `entity_uri_parse_default` rule matches
`EntityUri::parse(..)…unwrap_or_else(` only within one line, so the
`reactive_shell.rs` occurrence (wrapped across two lines by rustfmt) never
fired, and the `live_block.rs` one sat in `archlint/baseline.txt` as accepted
debt.

## Missing piece

No test fed a `live_block` an id that forms no URI. The sibling contract for
`rendered_text` / `editable_text` row ids was pinned by
`frontends/gpui/tests/render_spec_row_id_windowed.rs` in the D142.a fix, and
the same question was never asked of `live_block`, of the keyed row providers,
or of any id-less row.

Perception, secondarily: the `block("")` default produced no panic, no log and
no visible artefact. Nothing distinguished "two rows aliased onto one key" from
correct behaviour at any observable the suite reads.

## Keystone repro

The keystone cannot reproduce this, and the reason IS the coverage gap. Every
id the composed PBT mints is URI-safe by construction —
`crates/holon-integration-tests/src/pbt/generators.rs:1029` and `:1046` build
`EntityUri::block(&format!("block-{next_id}"))`, the `Create` transitions use
`gen-{next}` (`transitions/create_block_under_focus.rs:139`), and the org-write
transitions use the fixed `GEN_PLACEHOLDER` (`transitions/write_org_file.rs`).
No generator can emit an id containing a space, so no transition sequence in
the current catalog reaches the state.

Closing it properly means widening the id alphabet at those generators — and
then every downstream oracle has to agree on what an unusable id renders as.
That is a keystone change, not a crate change, and it is DEFERRED: this lane
pins the behaviour with the three tests below instead. Flagged so the next
keystone-generator pass picks it up.

## Remedy

FIXED (lane `block-ctor-sweep`).

- The DSL leg returns `ViewModel::error("live_block", …)` naming the id.
- Both `block_id`-prop legs parse once: the gpui builder paints
  `error_banner(..)` and creates no shell, and `collect_referenced_cache_keys`
  skips the key the builder will never create, so the two sides still agree.
- The three keyed providers `.expect(..)` — every row they mint carries an
  `id`, so absence is a bug and now says so.

Pinned by `frontends/gpui/tests/live_block_id_windowed.rs`, which locks BOTH
legs in the window — the DSL leg's message rides the `error` widget's
`error_message` tracker, the prop leg's rides the builder's own `error_banner`,
and the test reads both — by
`crates/holon-frontend/src/shadow_builders/live_block.rs`'s own view-model
tests one layer below, and by
`crates/holon-frontend/src/value_fns/synthetic.rs`'s tests (an id-less keyed
row is refused, an id-bearing one keeps its id). All three logged red first:
`lane-logs/g4b-red.log`, `lane-logs/g6-red-shadow-builder.log`.

Enforced by a new `archlint` rule `entity_uri_block_panics_on_bad_input`
rejecting `unwrap_or_else(|…| EntityUri::block(` and `EntityUri::block("")` in
production trees. It found two sites the hand pass missed
(`crates/holon-loro/src/loro_backend.rs`), both provably safe — the fallback
arm mints its own UUID — and now annotated.
