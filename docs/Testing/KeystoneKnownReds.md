# Keystone known reds — the full-depth registry

The per-weave land gate is `just keystone-smoke` (ONE proptest case). The
full-depth sweep (`just pbt general`, default 64 cases) reaches sequences the
smoke never draws, and today it fails roughly half its runs from the
pre-existing signatures below. That is why full depth runs as a NIGHTLY tier
(`just keystone-nightly`) instead of per-weave, and why the nightly is judged
against this registry rather than against "zero failures".

**The discipline:**

- A failure whose signature matches a row below is a **pass-with-note**: the
  nightly prints it as a `WARN` line and still exits 0.
- **ANY other signature is a regression to triage** — the nightly exits
  non-zero and prints the novel signature verbatim. Do not add a row to silence
  it; triage it first (`bug-gap-triage`), and only register it here if Martin
  ratifies it as a known red.
- A row is **removed** when its fix lands with a green soak (repeated
  full-depth runs no longer producing that signature). Rows are not archived
  in place — the registry describes what is red NOW.
- Between the fix landing and the soak confirming it, a row sits at status
  `fixed-pending-soak`. Only `known-red` rows classify, so from that moment a
  recurrence of the signature is reported as NOVEL — the fix is treated as
  believed, not proven, and the tier stops absorbing that signature silently.
  Delete the row once the soak is green; revert it to `known-red` (with the new
  payload decoded) if the soak brings it back.

**How the matching works:** `scripts/keystone-known-reds.sh` parses THIS FILE.
The `Match pattern` column is the single source of truth for classification —
an extended-regex (`grep -E`) applied to each extracted failure signature line.
Editing a pattern here changes the nightly's verdict; there is no second copy
in the script or the justfile. A pattern may not contain `|` — that is the
markdown table separator; use character classes instead of alternation.

## Registry

| Key | Status | Match pattern | Signature | Evidence | Task |
| --- | --- | --- | --- | --- | --- |
| `syn-real-mint` | known-red | `per-tick reconcile: one synthetic per minted real id` | `harness.rs` assertion `per-tick reconcile: one synthetic per minted real id (syn=[], real=[...])` — the per-tick synthetic→real id reconcile finds a real block minted with no synthetic counterpart. | Pre-existing since ≤2026-07-25; also fires in the windowed (GPUI) keystone, so it is not headless-only. | #62 |
| `org-render-echo-loop` | fixed-pending-soak | `diverged from the oracle: .*"inv-org-render-fixed-point"` | `[inv-org-render-fixed-point] render != disk PERSISTED for <budget> — … a real echo-loop / oscillation`. The decoded instance is a ONE-BYTE difference: disk 277 bytes vs rendered-from-SQL 278, identical except a blank line the renderer emits between a multi-line block body and the next heading, which the parser does not write back. | Decoded 2026-08-04 from `fixture-logs-2026-07-31/keystone-nightly-20260731-083505-run2.log.zst` (85 panics). SUPERSEDES the prior "reproduces around empty-title headings" attribution — that was never true of any captured payload. All 85 corpus panics are the same shape: render−disk = exactly +1 byte, all on `forward-edge-page.org`. **FIXED 2026-08-04**: `render_headline_block` (`crates/holon-org-format/src/models.rs`) pushed a second newline after every non-empty body; it is now emitted only when the body's last line starts a list item (`body_needs_list_terminator`). The unconditional REMOVAL was tried first and refuted by verification — that blank line is load-bearing org syntax after a list-like line, and without it a following `#+BEGIN_SRC` child is swallowed into the list and LOST on write-back → ingest (real data loss on the `FileFormatAdapter` seam). Red-first proof: `holon-orgmode` `render_of_parsed_disk_text_is_byte_identical` (the 277-byte corpus fixture) RED at 278 bytes → GREEN; `blank_line_closing_a_list_body_survives_round_trip` locks the other half. Closed gate gap: `org_block_round_trip_pbt` — the only binary driving the adapter seam — was in NO `just` recipe, which is why the refuted attempt passed every gate; `just pbt orgmode` now runs it. Status stays `fixed-pending-soak` until a full-depth soak confirms the signature is gone; the classifier already treats a recurrence as NOVEL. The regression lock is that unit test, NOT a keystone JSONL case: write-back and the `render_org` tool share one `WritebackRenderer`, so an ordinary edit converges disk and render by construction — three probe shapes (multi-line body + following sibling, on the seeded forward-edge page and on journals) all replayed GREEN with the defect deliberately re-added. Reproducing it composed needs a second, unrelated ingredient (a write-back that does not land — veto/quarantine or an incremental splice), so a keystone case would lock that conjunction and go red for another family's reason once this one is fixed; disclosed deviation, ruled 2026-08-04. | #66 |
| `org-blocks-ref-diverge` | known-red | `diverged from the oracle: .*"inv-blocks-match-ref/[a-z_]+".*fields diverge from reference` | `[inv-blocks-match-ref/org]` reports `fields diverge from reference`. Decoded instance: SUT-org 33 blocks vs reference 31, `only_in_ref` EMPTY, the two extras (`block:7-cn4c9-9js-vi026hjesuj2-3`, `block:b-5-fx-g-k3x54-7v-8x`) both parented to `block:ref-doc-2` — a set-membership excess on the SUT side confined to one externally-ingested ref-doc, NOT a field diff. | Decoded 2026-08-04 from `fixture-logs-2026-07-31/keystone-nightly-20260731-191108-run1.log.zst` (123 panics — the dominant family). SUPERSEDES the prior JOURNALS-PROJECTION attribution: no journals block appears in the divergence. | #76 |
| `split-id-no-pairing` | known-red | `resolve_sut_id: oracle-only id .* has no SUT pairing` | `types.rs` `resolve_sut_id: oracle-only id block::split-N has no SUT pairing … Mapped: [block::split-0, block::split-1, block::split-2]` — the first three editor splits reconciled, the fourth did not. The mirror signature of `syn-real-mint` (that row is a real id with no synthetic; this is a synthetic with no real). | 52 panics in `fixture-logs-2026-07-31/keystone-nightly-20260731-193535-run1.log.zst`. Registered 2026-08-04 — it had been firing unregistered, so the nightly could not issue a verdict. | #62 |
| `watch-rows-cdc-parent` | known-red | `diverged from the oracle: .*"inv-watch-rows-match-ref".*CDC parent_id mismatch` | `CDC parent_id mismatch for block:tpl-c1 in watch '<id>': actual_ui_model=Some("block:tpl") expected=Some("__document_root__")` — a template child whose CDC-delivered `ui_model` parent is the template block while the reference still holds the document-root sentinel. | 23 panics in `fixture-logs-2026-07-31/keystone-nightly-20260731-193535-run1.log.zst`. Registered 2026-08-04. Oracle-vs-prod verdict OPEN (plan F4). | #76 |
| `state-toggle-row-absent` | known-red | `could not resolve the state_toggle cycle intent` | `components.rs` `[toggle_state] click #1 failed for <block>: … in region main within 2s. <block> renders NO node in region main — the panel is not showing this block.` The reference believes a block is present and toggleable in the main panel; the panel does not render it. Same defect surface as `inv-main-panel-rows-match-focus`'s dropped-row arm, hit by the driver before the invariant can report it. | 23 panics in `fixture-logs-2026-07-31/keystone-nightly-20260731-191108-run2.log.zst`. Registered 2026-08-04. | #77 |
| `page-without-own-file` | known-red | `diverged from the oracle: .*"inv-every-page-has-its-own-file"` | `[inv-every-page-has-its-own-file] 1 page(s) not homed to exactly one own file: ["page <uuid> owns NO file (fileless — content lives only in th…"]` | 1 panic across the 2026-07-31 corpus. Registered 2026-08-04 for verdict completeness; a single lifetime observation, so soak absence alone must not close it. | — |
| `loro-stable-id-missing` | known-red | `missing STABLE_ID metadata` | `Node TreeID { peer: …, counter: … } missing STABLE_ID metadata` — a Loro tree node reached without the stable-id metadata the backend requires. Suspected same shallow-snapshot/history-trimming class as `loro-frontier-height`. | 1 panic across the 2026-07-31 corpus. Registered 2026-08-04. | #78 |
| `editor-caret-mirror` | known-red | `diverged from the oracle: .*"inv-editor-caret/mirror".*Caret mismatch` | `[inv-editor-caret/mirror] Caret mismatch on <block>: reference model cursor_byte=…, SUT tracked caret=…`. Only reachable in wirings that mirror the editor — engagement is 0 in Loro-only draws, so absence in a green run is not evidence of a fix. | Task #66 family, NEW 10. | #66 |
| `sidebar-focus-bind` | known-red | `LeftSidebar never bound a navigation.focus click-intent` | `components.rs` `[SutFocusWrite::apply_navigate_focus] LeftSidebar never bound a navigation.focus click-intent for <block> within 5s` — the sidebar's nested live_block watch fails to stream the target's selectable; latent arrival-order sensitivity, amplified (not created) by the reverted ORDER-BY snapshot change. | Fired 3× in a 64-case run at main-based tree 2026-07-31. | #77 |
| `pinblock-unrendered-target` | known-red | `PinBlock.sql_reads: [0-9]+ exceeds expected 17` | `[inv-sql-budget PINNED] PinBlock.sql_reads: N exceeds expected 17 + tolerance 1 = 18`, N ≈ 90–141, wall ≈ 2.2s, spans ≈ 520–820. Co-fires with `inv-focus-roots` (right_sidebar) and `inv-main-panel-rows-match-focus`. | Ledger entry 14, RESOLVED 2026-08-04 by the measurement lane (+ verifier round): NOT a symptom of the focus-roots red and NOT a cost-model gap — all three reds share ONE cause, a pin target whose NESTING DEPTH exceeds the main-panel query's depth-20 recursion cap (`WHERE _vl2.depth < 20 … AND _vl2.depth <= 20`, documented at `crates/holon/tests/turso_storage_repros/tabs_main_panel_delivery.rs:130`). Past the cap the panel renders no row for it, so `click_entity_with_modifiers` (`user_driver.rs:719`) spins its 2s poll — 41 redundant re-snapshots of two `watch_view` SELECTs — and the pin never dispatches. Depth 12 → 17 reads; 21 → 89; 22 → 101. Width is irrelevant: a 40-block FLAT panel renders all 40 rows and pins the 40th for 17 reads. Oracle side, `main_editable_descendants()` applies no depth filter, so the generator offers targets the panel query truncates. Do NOT widen the ceiling. | #7 |
| `loro-frontier-height` | known-red | `frontier change present` | Panic at `loro_backend.rs` `doc_lamport_height`: `doc.get_change(id).expect("frontier change present")` — a frontier id whose change is not retrievable (suspect: shallow-snapshot history trimming). | Fired 1× in a 64-case run at main-based tree 2026-07-31. | #78 |

## Evidence corpus & the pattern-drift guard

Every row's Evidence column must name a **decoded payload**, not a recollection.
Two rows were found on 2026-08-04 to describe something no captured payload ever
showed (`org-render-echo-loop` blamed empty-title headings; `org-blocks-ref-diverge`
blamed the journals projection), and both would have mis-steered a fix. So the
raw logs those verdicts came from are committed, zstd-compressed, at
`crates/holon-integration-tests/hand-authored-regressions/fixture-logs-2026-07-31/`
— four red runs and the four green runs of the same nights (the 4/8 = 50%
base-rate record). They are evidence, not scratch: `/tmp` does not survive a
reboot.

`just known-reds-fixture` replays the four red logs through
`scripts/keystone-known-reds.sh` and asserts the per-key hit counts are exactly
what the corpus contains. Since the corpus is immutable, a diff there means a
`Match pattern` above no longer matches the text it was written for — i.e.
someone reworded an assertion message and silently broke the classifier.
Re-bless with `scripts/keystone-known-reds-fixture.sh --bless` **only** after
confirming the pattern change is intended.

Run it whenever you touch a panic/assertion message that a pattern above quotes.

## Where it runs — local, not GitHub Actions

The tier is a LOCAL nightly (Martin's machine or an orchestrator session). No
scheduled workflow was added, because CI cannot currently execute the composed
keystone at all:

- `.github/workflows/ci.yml`'s `rust-checks` job runs
  `cargo test --workspace --exclude rust_lib_holon` on `ubuntu-latest`, and
  `pbt` IS a default feature of `holon-integration-tests`, so in principle the
  keystone is in that job's scope.
- In practice it never gets there. The last 200 CI runs are 200 failures; the
  step spends ~14min compiling and then dies inside the `holon` crate's own
  suite (`create_page_from_link`), before any `holon-integration-tests` binary
  starts. `general_e2e_composed_pbt` appears in ZERO CI logs.
- Full depth is hours of wall clock on top of that compile, on a 2-core runner.

A scheduled job today would be a gate that never ran the keystone. Re-evaluate
once CI is green and the runner budget is measured; until then
`just keystone-nightly` IS the tier.

## Running the tier

```
just keystone-nightly            # 2 serialized full-depth runs, judged against this file
just keystone-nightly 1 8        # 1 run at 8 cases — for exercising the plumbing, not a gate
```

Keystone runs must be serialized against every other keystone lane:

```
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-keystone -j1 --fg -- just keystone-nightly
```
