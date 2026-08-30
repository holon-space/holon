# Design: Web arm for the keystone PBT (dioxus-web under test)

Status: RATIFIED by Martin 2026-08-15 — D3.a (in-Rust CDP: chromiumoxide or
fantoccini/WebDriver, one language, no sidecar), D4.a (dual oracle: MCP relay
for authoritative state + quiescence, DOM for rendered-set), D5.a
(keystone-web-smoke in landing-gate + prepush, ~5 fixed cases + hand-authored
replay). The spike lane lands this doc into docs/Testing/ with increment 1.

## 1. Goal & first principles

- **Goal:** the composed keystone PBT can drive the dioxus-web frontend as its
  SUT, so the browser stack (DOM event loop → dioxus → wasm worker → engine →
  OPFS) is covered by the same generated interaction corpus as headless and
  GPUI. Today NO gate even boots this stack deep enough to see a dead rule
  engine (task #38's escape, BugFunnel ENVIRONMENT row, 2026-08-15).
- **North star (standing directive):** ONE env-selected PBT — the web arm is a
  new driver/environment for the existing keystone, never a separate test
  (memory: north-star; dedicated-pbts-share-keystone-structure).
- **Cost frame (measured):** windowed GPUI runs ~100x slower than headless
  (~2min vs ~1.1s per test). A browser arm lands in the same cost class, so it
  enters as a thin smoke tier that proves WIRING; headless stays the volume
  bug-finder (memory: holon-test-tier-strategy).
- **What only this arm can prove:** browser-platform wiring — worker boot and
  watcher startup, OPFS persistence across reload, COI/SAB preflight, DOM
  event→intent mapping, service-worker/caching behavior. Every one of these
  has already produced a real escape (tasks #38, #41, incognito disclosure).

## 2. What exists (verified 2026-08-15 at main fa114fdc)

- `trait UserDriver` — crates/holon-frontend/src/user_driver.rs:90. User verbs
  (`send_key_chord`, `click_entity`, `replace_text`, `drop_entity`) plus
  `synthetic_dispatch`/`apply_intent`. Impls today: `ReactiveEngineDriver`
  (headless, :836), `DirectUserDriver` (mutation_driver.rs:236),
  `McpUserDriver` (mcp_user_driver.rs:232), `WebUserDriver`
  (crates/holon-integration-tests/src/web_user_driver.rs, increment 1),
  Flutter via Dart callbacks. There is no `type_text` verb: typing goes
  through `replace_text`, whose default impl composes `click_entity` +
  `send_raw_keystroke`, so a driver gets it by implementing the raw
  keystroke.
- MCP relay: the browser dials OUT to a local hub (`connect_mcp_relay`,
  frontends/dioxus-web/src/main.rs ~507; wss+capped retry landed in fa114fdc).
  With a local hub running, an external process can speak MCP to the live
  browser engine — the observation channel already exists.
- Local serve recipe proven by t38b–t38e and reused by increment 1:
  `trunk build` in frontends/dioxus-web → `node frontends/dioxus-web/serve.mjs`
  (COOP/COEP, resolves the worker wasm from cargo target/ or
  `HOLON_WORKER_WASM`) → fresh browser context → drive/assert.
- Entity addressing in the DOM: the dioxus renderer emits
  `data-entity-id` (the scheme-qualified uri, e.g. `block:welcome`) on
  `rendered-text`, `editor-cell` and — added by increment 1 —
  `selectable`, plus `data-boot-state` on the title bar for the readiness
  gate.

## 3. Architecture

One new driver, two channels, behind the existing trait:

```
keystone proptest (native cargo test, HOLON_PBT_ENV=web)
   │
   ├── interaction channel: WebUserDriver
   │      real-gesture verbs → browser automation (D3: CDP-in-Rust vs
   │      Playwright sidecar) → real DOM events → dioxus → worker
   │      synthetic_dispatch → reuse McpUserDriver over the relay hub
   │
   └── oracle channel (D4): MCP relay reads authoritative projections +
          quiescence-await; DOM snapshot for the rendered-set invariants
```

- **Interaction goes through the DOM, never MCP.** MCP-injected intent skips
  the UI wiring layer — precisely where the escapes live. MCP stays sanctioned
  for `synthetic_dispatch` (the trait already documents synthetic dispatch as
  a fallback, and `McpUserDriver` is written).
- **Reset per case:** new browser context per proptest case → OPFS wiped for
  free. Server process + built dist/ are per-suite fixtures, not per-case.
- **Quiescence:** await the engine via MCP (projection generation / pending-op
  count) instead of sleeps — same role SpanCollector plays headless.

## 4. Increments (each independently landable)

1. **Spike:** WebUserDriver skeleton with 3 verbs (click_entity, type_text,
   send_key_chord) + one hand-authored regression from
   hand-authored-regressions/keystone.jsonl replayed green in the browser.
   De-risks D3 empirically (auto-wait quality, per-op latency measurement).
2. **Oracle:** MCP-relay oracle adapter + rendered-set read from DOM;
   keystone invariants evaluated on web arm; red-first proof = revert the
   worker watcher fix in a scratch tree → the arm must go red for that reason
   (this is the PBT that task #38's escalation said was missing).
3. **Gate:** `just keystone-web-smoke` (N small cases, fixed seed corpus +
   hand-authored replay) wired per D5; BugFunnel rows for anything the arm
   catches during bring-up.
4. **Later:** reload-persistence transition (close context, reopen, assert
   state survives via OPFS) — the one interaction class no other arm can even
   express.

## 4a. Increment 1 result (measured 2026-08-15)

D3.a holds. Transport is `chromiumoxide` 0.9.1 (pinned `=0.9.1`, behind the
`web-arm` feature of `holon-integration-tests`); `fantoccini` was rejected
because it needs a chromedriver binary version-matched to the local Chrome,
which is the sidecar D3.a rules out. Chrome launches, an incognito browser
context gives a clean OPFS per case, and real clicks / keystrokes drive the
app. Evidence: lane-logs/webpbt-spike-run5.txt.

Measured over 30 ops (`--features web-arm --test web_arm_spike`):

| | p50 | p95 | max |
|---|---|---|---|
| wall per op (incl. 300ms settle padding) | 316ms | 479ms | 858ms |
| gesture → last DOM change | <20ms | 151ms | 211ms |

Case reset (fresh context + boot + OPFS-clean assert): ~2.5s.

The wall figure is a harness knob, not app latency: with no engine-side
quiescence signal in increment 1, each verb waits out a 300ms settle window
(`HOLON_WEB_SETTLE_MS`). The number that speaks to the >200ms/op risk is the
effect latency, which stays at or under ~210ms — so cases need not be capped
below the design's 15-op guidance, and increment 2's MCP quiescence should
remove most of the padding.

**Auto-waiting finding (feeds D4.a):** DOM stability alone is NOT a sound
quiescence oracle here. The worker advances on a 16ms tick pump, so the DOM
sits unchanged mid-flight; a two-equal-reads rule declared a block split
settled and the harness observed the new block only on the *next* gesture.
The settle window is the increment-1 stand-in; the MCP-relay quiescence D4.a
already rules for is the real fix, and this is empirical support for it.

## 4b. Increment 2 result (measured 2026-08-16)

D4.a holds, and the relay quiescence removes the padding §4a predicted it
would.

- **Primary signal is the relay.** `WebUserDriver::await_quiescence` now calls
  the browser engine's `await_quiescence` tool (CDC watermark, 50ms unchanged
  window) and only then waits for the DOM — for `HOLON_WEB_RENDER_WINDOW_MS`
  (60ms default) instead of the 300ms `HOLON_WEB_SETTLE_MS` window, because a
  converged engine can produce no further work. The DOM window is retained as
  the disclosed secondary; a relay failure is a hard error, not a silent
  fallback to the weaker signal.
- **Measured:** engine convergence 133–221ms per gesture; click+`end`+`enter`
  (3 gestures, 3 relay round-trips, 3 render windows) 665–843ms wall. The
  spread is machine load, not variance in the app: the low end is an idle
  machine, the high end the same assertion with four sibling lanes compiling.
  Increment 1's comparable figure was 316ms p50 for ONE op, i.e. ~950ms for
  three. Treat these as an order-of-magnitude check, not an SLO measurement —
  in-browser performance measurement is explicitly out of scope (§6).
- **Dual oracle, three points.** `web_arm::read_and_cross_check` asserts DOM
  rendered-set ⊆ engine `debug_pbt_snapshot`; that the `block_raw` row COUNT is
  at least the number of live blocks the block query reports (raw SQL, one
  layer below the projection — a count, NOT a set subset, because `block_raw`
  also holds rows the block query filters out, so only that direction is sound
  without re-implementing the filter in the harness); and rendered text against
  committed content. The split test then requires all three to move together:
  engine 20→21, DOM 3→4, `block_raw` 21→22.
- **Red-first proof.** Commenting out `start_action_watchers` in the worker's
  boot and rebuilding the wasm turns
  `web_arm_rule_engine_materializes_the_day_page` red for exactly its reason
  (19 blocks, no block carrying today's date, the `daily_journal` rule block
  present but never fired); restoring and rebuilding turns it green.
  `lane-logs/webpbt-inc2-red.log` / `-green.log`.

**Keystone replay was BLOCKED by a defect the arm found, now fixed.** The
corpus is authored over the wide seed (`block:parent`/`c1`/`c2`), which a
browser can only get from the `reset_vault` tool — and that tool left the live
page bound to the torn-down engine (BugFunnel 2026-08-16, ENVIRONMENT), so its
blocks never became gesture-reachable. The rebind lands with
`web_arm_reset_vault_rebinds_the_live_page`: the worker publishes an engine
generation on every swap, the page's tick pump detects the change and re-runs
its root-layout bind against the new engine. A bind is a chain of awaits and
callbacks that a reset can land inside, so each one carries an epoch and its
continuations go inert once superseded — without that, a bind the page had
moved past killed the rebound page ten seconds later (BugFunnel
2026-08-31-superseded-bind-watchdog-kills-rebound-page), which is what
`web_arm_superseded_bind_cannot_kill_the_rebound_page` and its spaced-reset
control now pin. Remaining cap:
`CreateBlockUnderFocus` pins the created block's id, which no gesture can
choose, so it needs the composed harness's synthetic→real reconcile.

## 4c. What increment 3 still needs

The replay leg is wired end to end against the keystone's own loader but still
replays zero cases, because its per-case `boot()` opens a fresh context on the
OPFS boot vault, which `seed_default_layout` fills — not the wide seed. Two
steps close it:

1. Per-case `reset_vault` after launch, to install the wide seed the corpus is
   authored over.
2. Navigate to `block:structural-page` before the transitions, since
   `block:parent`/`c1`/`c2` are its children and a gesture only reaches a block
   the renderer has mounted. `web_arm_reset_vault_rebinds_the_live_page` does
   both and is the recipe.

Then the D5.a gate wiring (`just keystone-web-smoke`, landing-gate + prepush).

## 5. Risks

- **Flake:** browser timing under CI load; mitigated by quiescence-await, no
  sleeps, low case count, retry-free assertions. The windowed tier's
  serialization lesson applies (cannot parallelize cases against one server —
  but CAN run k contexts against one server if OPFS is per-origin-context;
  verify in spike).
- **Per-op latency dominates:** if spike measures >200ms/op, cases must stay
  short (≤15 ops); shrinking budget capped; rely on hand-authored replay for
  determinism (jsonl-regressions-not-seeds directive).
- **Divergent caps:** web arm initially won't support every keystone
  transition (drag&drop fidelity, multi-window). Per fix-cap-not-withhold:
  caps are declared, tracked, and burned down prod-faithfully, not hidden.
- **Cross-language seam (if D3 = sidecar):** version skew + JSON bridge
  maintenance inside the test suite.

## 6. Out of scope

- Mobile webviews; visual/pixel assertions (stay windowed-GPUI concerns);
  replacing dogfood-explorer (it remains the final exploratory gate);
  performance SLO measurement in-browser (separate lane, cold-boot 20s watch
  item from t38e).
