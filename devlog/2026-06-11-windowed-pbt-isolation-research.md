# Why windowed PBT "needs an idle machine" — verified mechanics + escape routes

Date: 2026-06-11 evening. Triggered by the user questioning the idle-machine
rule ("I don't think we had this issue before"). Two research agents (general
macOS isolation; Zed/GPUI internals) + empirical data from today's runs.

## What is actually true (hypothesis decomposition)

| Claim | Verdict | Evidence |
|---|---|---|
| H1: synthetic input is dropped when window not key | **FALSE** | `Window::dispatch_keystroke` (gpui window.rs:4197) never checks key/active status — in-process dispatch routes to the focused element regardless. Empirical: tonight 4 runs with 280–360 `CONTAMINATION` markers each were all GREEN; the one red run had ZERO markers. |
| H2: key loss changes app behavior (blur) | **TRUE** | On `windowDidResignKey`, gpui flips `window.active`; next render fires focus listeners with an EMPTY focus path (window.rs:2431-2440) → focused editor blurs. Pre-Phase-A this caused real data loss (blur-bimodality bug); post-Phase-A (commit-on-authority-move) it should be benign — de-key experiment quantifies this. |
| H3: direct HID interference | TRUE but narrow | Only while the user actively types/clicks; user keystrokes can land in the test window if it grabs key status. |
| H4: occluded window stops painting | **TRUE, and our pump comment is wrong** | `window.refresh()` only sets dirty flags; drawing is driven by CVDisplayLink, which macOS STOPS for fully-occluded windows (`window_did_change_occlusion_state` → `stop_display_link`). Our "RPC doubles as frame pump" works only because the test window stays visible (non-key ≠ occluded). |

Correction to harness: the `[interaction-pump] CONTAMINATION` text claims
"synthetic input may be dropped" — wrong; should say "window-active flipped:
blur side-effects fire / display-link may pause if occluded".

Key fact from AppKit: key-window status is GLOBAL (one per desktop, in the
active app). There is no public way for a background app's window to be key.

## Escape routes, ranked for this project

1. **Migrate windowed PBT to gpui's TestPlatform** (what Zed itself does — they
   run ZERO real-window tests in CI). Same `dispatch_keystroke`/`dispatch_event`
   code paths, no NSWindow/CVDisplayLink, seeded deterministic scheduler
   (`TestDispatcher`), `run_until_parked()` replaces settle-polling,
   `deactivate_window()` fires blur DETERMINISTICALLY (we can test the blur
   path on purpose!), optional `MetalHeadlessRenderer` for pixel-real
   offscreen rendering. Kills the idle-gate apparatus AND makes
   runner-coupled Heisenbugs seed-reproducible. Keep a tiny real-window smoke
   suite for true platform behaviors.
2. **Tart VM** (Virtualization.Framework) for whatever real-window suite
   remains: own WindowServer, test app holds key status indefinitely, host
   user fully isolated. ~30s boot / few GB RAM; scriptable.
3. **GitHub Actions macOS runners** have a full GUI session — real-window
   tests CAN run in CI (TCC pre-seeding needed). Good for a nightly gate.
4. CGVirtualDisplay (+ private SkyLight `SLPSPostEventRecordTo`) can solve
   occlusion (+ key-status without focus-steal) on the dev laptop — but
   private APIs; not worth it given option 1.
5. Fast user switching does NOT work (inactive session = display asleep,
   display links pause).

## De-key experiment result (21:09)

4 runs with a Finder-activate-every-3s loop. Two HID-clean: run1 RED, run2
green. run1's failure = `inv-displayed-text` after TypeChars: editor shows
"xgafU7BNx6V8", ref expects "U7BNx6V8xgaf" — the typed chunk landed at END,
i.e. the deactivation blur re-seeded the caret mid-typing-dance. NO
inv-blocks-match divergence: data loss stays gone even under constant
de-keying (Phase A holds). Conclusion: key-status churn now affects only
CARET-PLACEMENT faces (~1/2 runs under 3s-interval de-keying), not data.

## Immediate consequences

- If the de-key experiment (4 runs, Finder-activate every 3s, HID idle)
  stays green → relax the idle rule to "no active typing in another app",
  keep the per-run idle verdict only as metadata.
- Fix the CONTAMINATION wording.
- TestPlatform migration is the strategic ticket — sized separately; the
  PBT harness already abstracts the driver (UserDriver / interaction pump),
  so the seam exists.
