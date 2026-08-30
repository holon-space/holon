---
id: 2026-08-31-root-layout-collections-never-stream
date: 2026-08-31
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Collections in the ROOT layout tree never had their reactive drivers
  started, so the mobile action bar's operation row was permanently empty
  on every platform.
---

## Bug

Found while wiring Martin's above-the-keyboard action bar (2026-08-30), by
driving a real GPUI window at a phone width — not by any automated test.

The bar's whole declarative pipeline was already in the tree and looked
correct: every perspective synthesizes
`if_space(600, bottom_dock(columns({narrow}), <ops collection>), …)`
(`crates/holon-api/src/perspective.rs:311`), the GPUI builder registry
dispatches `bottom_dock` and `op_button` by widget name
(`frontends/gpui/src/render/builders/mod.rs:30`), and `chain_ops(0)`
projects the focused block through the operation catalog
(`crates/holon-frontend/src/value_fns/chain_ops.rs`). The window painted a
`bottom_dock`. It painted zero operations, and had done so since the widget
landed.

Evidence, from a windowed run at 393x852 with a block focused
(`lane-logs/ab-diag-2.log`, `ab-diag-4.log` in the lane):

```
[XXDIAG] chain_ops INVOKE level=0 has_focus_authority=true   ×3
                                    ← and no build_rows call at all
```

The value function was invoked and produced a provider; the provider's rows
were never once pulled.

## Root cause

Three defects, all in the same never-rendered path. The first is the one
that generalizes:

1. **The root layout's drivers are never started.** A `ReactiveView` owns
   its data pipeline and spawns it in `ReactiveView::start`, which
   `start_reactive_views` walks a tree to call
   (`crates/holon-frontend/src/reactive_view.rs:2560`). Every consumer of a
   freshly interpreted tree calls it — `watch_live`
   (`crates/holon-frontend/src/reactive.rs:2686`), the per-block
   `ReactiveShell` on each structural change
   (`frontends/gpui/src/views/reactive_shell.rs:208`), the view-mode
   switcher — except the root. `spawn_root_layout_signal` took the
   `ReactiveViewModel` off `engine.watch_signal(root_uri)` and stored it
   straight into `AppModel::root_vm`. Any collection in the root layout was
   therefore born with no driver and stayed empty forever. Nothing had
   noticed because the action bar's ops row is the only collection the root
   layout has ever carried.

2. **The dock slot collapsed to zero height.** With a driver running, the
   ops rendered through the default collection path — a `ReactiveShell`
   under `scrollable_list_wrapper`'s `size_full` chain — inside the dock's
   intrinsic-height box, which resolves to 0. Measured: every button at
   `y=512.0 w=0.0 h=0.0`.

3. **The bottom inset was applied twice.** `HolonApp::render` pads the page
   container by `safe_area_bottom` (`frontends/gpui/src/lib.rs:1551`) and
   `bottom_dock` padded itself by a second, independent read of
   `crate::mobile::safe_area_bottom_px()`. On a phone with the keyboard up
   that is 2× a ~290px inset. The second read also meant
   `RebindHandle::set_safe_area_bottom` — the seam every windowed
   keyboard-inset test drives — could not reach the dock at all.

## Missing piece

No test ever rendered the root layout's dock slot end to end. The shadow
tests (`crates/holon-frontend/tests/bottom_dock.rs`) assert the
`bottom_dock` NODE exists at a narrow viewport and stop there — they
interpret with `StubBuilderServices`, which has no focus authority, so
`chain_ops` returns a fixed empty row set and the whole streaming half of
the widget is unobservable to them. The windowed tests that DO drive a real
session all ran at desktop widths, above the `if_space(600, …)` breakpoint
where the dock branch is never taken. Between the two, the bar's only
rendering path had no coverage at any tier.

## Remedy

Fixed, with the gap closed first: `frontends/gpui/tests/action_bar_windowed.rs`
drives a real window at 393x852 with a grafted outline and went red for the
right reason (`<no op_button painted>`) before any of the three fixes, at
`lane-logs/ab-red-1.log`.

- `spawn_root_layout_signal` now calls `start_reactive_views` on every root
  tree it receives, matching what `ReactiveShell` does for the trees it owns.
- `bottom_dock` renders a collection dock slot through
  `column::eager_collection_div` — content height, the same firewall
  content-sized columns use — and the dock slot's DSL became a `horizontal`
  `list`, the construct the settings integration rows already prove paints
  op buttons along one line.
- The page container is now the sole owner of the bottom inset, which is
  correct: it must pad by the TOTAL unusable strip. The dock applies no
  padding and instead gates on a separate `KeyboardHeight` global — the
  keyboard's own height, republished from `HolonApp` each frame and driven in
  tests through `RebindHandle::set_keyboard_height`.

  Splitting the two signals is the load-bearing part. An earlier version of
  this fix gated the bar on the bottom INSET being non-zero, which reads
  correct on a desktop window (it rests at 0) and is wrong on every phone: the
  inset also carries the home indicator, nav bar and gesture area, so it is
  never zero and the bar would have been permanently on screen. Layout wants
  the total; "is the keyboard up" wants the keyboard alone.

  `gpui_mobile::keyboard_height()` is the cross-platform accessor for that
  second question, and iOS is the only platform that feeds it from its own
  window. Android's producer is a fork change — `refresh_keyboard_height` in
  `src/android/jni.rs`, reading
  `decorView.getRootWindowInsets().getInsets(WindowInsets.Type.ime())` (API
  30+, fail-loud below that since minSdk is 33) and publishing through the same
  `set_keyboard_height`. Holon gets the Android leg once `Cargo.lock` pins a
  gpui-mobile rev containing it; check the pinned rev before reading this as
  working on Android.

The keystone cannot reproduce any of this: it has no window, so the
breakpoint branch, the inset, and the zero-height collapse are all
structurally invisible to it. The windowed rungs are the lowest tier that
can see them.

Residual: Android polls the IME inset at content-rect events rather than
subscribing via `setOnApplyWindowInsetsListener`, so a frame sampled during
the show/hide animation can read a partial height. Cosmetic — the bar's gate
is "non-zero", which a partial height already satisfies.
