# GPUI Frontend Testing Strategy

## The problem: `run_until_parked()` hides frame-timing bugs

GPUI's `TestAppContext::run_until_parked()` drains **all** pending async work,
notifications, and render passes in one atomic step. There are no frame
boundaries. The test never observes intermediate state.

The real macOS event loop works differently — it processes work in frame cycles:

```
Frame N:
  1. Handle input event (button click → data.set() + cx.notify() on parent)
  2. Render dirty entities (parent re-renders; children aren't dirty yet → cached stale output)

Frame N+1:
  3. Poll async tasks (signal fires → children update → cx.notify() on children)
  4. Render dirty entities (children re-render with fresh data)
```

Bugs that live in the gap between frames 2 and 4 are invisible to
`run_until_parked()` because it collapses the entire sequence into one step.

## Three test tiers

### Tier 1: Headless tests (`reactive_vm_test.rs`)

Uses `gpui::run_test_once` + `TestAppContext`. Fast, deterministic, no window.

```
cargo test -p holon-gpui --test reactive_vm_test
```

**Good for**: signal wiring, structural changes, data propagation, trait boundaries.

**Blind spot**: can't detect frame-timing bugs because `run_until_parked()` drains
everything atomically.

**Pattern — standard signal test:**
```rust
run("my_test", |cx| {
    let node = cx.new(|cx| ReactiveNode::new(expr, data, interp(), cx));
    cx.run_until_parked();           // initial render

    node.read_with(cx, |r, _| {
        r.data.set(Arc::new(new_data));
    });
    cx.run_until_parked();           // drains signal + re-render atomically

    // Assert on final state — intermediate states are not observable
    let snap = cx.read(|cx| node.read(cx).tree_snapshot(cx));
    assert!(snap.leaf_displays()[0].contains("new_value"));
});
```

### Tier 2: Headless invariant tests (`reactive_vm_test.rs`)

Same harness as Tier 1, but tests the **data flow invariant** directly: after
`data.set()` but **before** `run_until_parked()`, prove that the render path
produces fresh output.

**Good for**: catching "`render()` reads stale Mutable instead of live data" bugs.

**How it works**: between `data.set()` and signal propagation, the `display`
Mutable is stale while `data` is fresh. The test asserts that the render path
(`interpreter.interpret(expr, data)` / `tree_snapshot()`) uses the fresh one.

**Pattern — invariant test:**
```rust
run("render_uses_live_data_not_stale_display", |cx| {
    let node = cx.new(|cx| ReactiveNode::new(expr, data, interp(), cx));
    cx.run_until_parked();

    // Set data — signal has NOT fired yet
    node.read_with(cx, |r, _| {
        r.data.set(Arc::new(make_row("1", "Fresh!", "low")));
    });
    // DO NOT call run_until_parked here

    // Prove the gap exists
    let (display, data_val) = node.read_with(cx, |r, cx| {
        let child = r.children[0].read(cx);
        (child.display.get_cloned(), child.data.get_cloned())
    });
    assert!(display.contains("Original"));  // stale — signal hasn't fired
    assert!(data_val.get("content").unwrap()
        .to_display_string().contains("Fresh!"));  // fresh — Mutable is synchronous

    // tree_snapshot uses interpreter.interpret(expr, data) — same path as render().
    // If this is fresh, render() is fresh.
    let snap = cx.read(|cx| node.read(cx).tree_snapshot(cx));
    assert!(snap.leaf_displays()[0].contains("Fresh!"));
});
```

### Tier 3: Real-window tests (`reactive_vm_realwindow_test.rs`)

Uses `Application::run()` on the main thread — the real macOS event loop with
real frame cycles. A background thread mutates state and polls for re-renders.

```
cargo test -p holon-gpui --test reactive_vm_realwindow_test
```

**Good for**: catching bugs that only manifest with real frame timing — missing
`cx.notify()`, signal tasks not being polled, cross-executor issues.

**How it works**: the background thread sets data via `Mutable::set()`
(thread-safe), then polls `render_count` (an `Arc<AtomicUsize>` incremented in
`render()`) until it increases. If `cx.notify()` is missing, the render count
never increases and the test times out.

**Pattern — real-window test:**
```rust
fn main() {
    let (handles_tx, handles_rx) = sync_channel::<Handles>(1);
    let (quit_tx, quit_rx) = sync_channel::<()>(1);

    // Background thread: assertions against real frame cycles
    let test_thread = std::thread::spawn(move || {
        let h = handles_rx.recv_timeout(Duration::from_secs(30)).unwrap();

        // Wait for initial render (poll render_count)
        while h.render_counts[0].load(Ordering::Relaxed) < 1 {
            std::thread::sleep(Duration::from_millis(50));
        }
        let before = h.render_counts[0].load(Ordering::Relaxed);

        // Mutate data (thread-safe, no GPUI context needed)
        h.data_m.set(Arc::new(new_data));

        // Wait for re-render through real frame cycles
        let deadline = Instant::now() + Duration::from_secs(10);
        while h.render_counts[0].load(Ordering::Relaxed) <= before {
            assert!(Instant::now() < deadline, "re-render timed out");
            std::thread::sleep(Duration::from_millis(50));
        }

        quit_tx.send(()).unwrap();
    });

    // Main thread: real GPUI event loop
    let app = Application::with_platform(gpui_platform::current_platform(false));
    app.run(move |cx| {
        cx.open_window(opts, |_, cx| {
            let root = cx.new(|cx| ReactiveNode::new(expr, data, interp(), cx));
            handles_tx.send(extract_handles(&root, cx)).unwrap();
            cx.new(|_| OpaqueWrapper { root })
        });
        // Poll for quit signal
        cx.spawn(async move |cx| { /* ... */ }).detach();
    });

    test_thread.join().unwrap();
}
```

## Which tier catches which bug?

| Bug | Tier 1 (headless) | Tier 2 (invariant) | Tier 3 (real-window) |
|-----|-------------------|-------------------|---------------------|
| Signal wiring broken | catches | N/A | catches |
| Missing `cx.notify()` in signal task | catches via render_count | N/A | catches (timeout) |
| `render()` reads stale display Mutable | **misses** | catches | catches |
| Cross-executor waker issue | **misses** | **misses** | catches |
| Structural apply_expr logic | catches | N/A | N/A (overkill) |

## When to use which tier

- **Writing a new reactive pattern**: start with Tier 1 for the logic, add a Tier 2
  invariant test if the pattern involves a render path that could read stale data.
- **Debugging a visual bug**: if Tier 1 tests pass but the UI is wrong, write a
  Tier 3 test — the bug is likely frame-timing or cross-executor.
- **Refactoring render()**: always run Tier 2 to verify the render data path
  doesn't regress to reading stale Mutables.

## Thread-safe handles for Tier 3 tests

`Mutable<T>` from futures-signals is `Send + Sync` (backed by `RwLock`). These
can be read/written from the background thread without GPUI context:

- `data.set(value)` / `data.get_cloned()` — read/write data
- `display.get_cloned()` — read computed display string
- `render_count.load(Ordering::Relaxed)` — check if render() was called

`Entity<T>` is NOT thread-safe — reading entity fields requires `&App` context
(main thread only). Extract the thread-safe handles during window creation and
send them to the background thread via channel.

## Key files

| File | Tier | Purpose |
|------|------|---------|
| `tests/reactive_vm_test.rs` | 1 + 2 | 17 headless tests including invariant tests |
| `tests/reactive_vm_realwindow_test.rs` | 3 | Real macOS event loop, polls render_count from background thread |
| `tests/gpui_ui_pbt.rs` | 3 | Full PBT with real window, BoundsRegistry, xcap screenshots |
| `src/reactive_vm_poc.rs` | — | Shared types: ReactiveNode, ItemNode, Interpreter, TreeSnapshot |
