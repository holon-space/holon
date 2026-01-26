---
name: futures-signals
description: Reactive FRP library used in holon-frontend for Signal/MutableVec-based streaming
type: reference
source_type: url
source_id: https://docs.rs/futures-signals/latest/futures_signals/
fetch_timestamp: 2026-04-23
---

## futures-signals (v0.3)

**Purpose**: Zero-cost functional reactive programming library providing reactive value abstractions with automatic change propagation.

### Key APIs

| Type | Role |
|------|------|
| `Signal<T>` | Core reactive value; propagates updates automatically |
| `SignalExt` | Combinators: `map`, `filter`, `fold` |
| `MutableVec<T>` | Mutable reactive collection |
| `SignalVec<T>` | Emits diffs (add/remove/clear) when collection changes |
| `Mutable<T>` | Shared mutable value that can be observed |
| `map_ref!` / `map_mut!` | Macros to combine multiple signals |
| `CancelableFuture` | Async operations with cancellation support |

### Integration in Holon

- **holon-frontend**: `MutableVec` drives UI list components; `Signal` models reactive state in `ReactiveViewModel`
- Pairs with GPUI's render cycle — signal changes trigger GPUI entity re-renders
- Used alongside futures-signals' `to_stream()` adaptor to bridge to `tokio` async streams

### Keywords
reactive, FRP, signal, streaming, MutableVec, change propagation
