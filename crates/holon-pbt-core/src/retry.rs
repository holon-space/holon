//! Generic "retry an async predicate until it succeeds or a deadline passes".
//!
//! Not specific to any one surface: it's the eventual-consistency primitive for
//! any read that may settle slightly after the action that caused it — a
//! CDC-fed Turso IVM matview, a file-watcher round-trip, a focus mirror, a
//! geometry-bounds promotion. The *why it can lag* belongs at each call site;
//! this just owns the timing. A surface that never settles still fails loudly
//! once the deadline elapses (no `Skip`, no masking).

use std::time::Duration;

/// Re-run `attempt` until it returns `Ok`, or `timeout` elapses (sleeping
/// `interval` between tries). Returns the first success value, or the last
/// `Err` on timeout.
///
/// `attempt` is an async closure so it can re-read the system under test on
/// each try. It returns `Result<T, E>`: `T` is whatever the caller wants out
/// of a successful read (often `()` for a pure assertion); `E` carries the
/// diagnostic to surface if the deadline is hit.
pub async fn retry_until_ok<T, E>(
    timeout: Duration,
    interval: Duration,
    mut attempt: impl AsyncFnMut() -> Result<T, E>,
) -> Result<T, E> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(e);
                }
                tokio::time::sleep(interval).await;
            }
        }
    }
}

/// Like [`retry_until_ok`], but between tries it awaits the caller-supplied
/// `wake` future (e.g. `GeometryProvider::changed`) capped at `max_wait` — the
/// loop wakes the moment the read source signals a change instead of on a
/// fixed timer. The cap covers a notification firing between the failed
/// attempt and the await (degrades to a `max_wait` poll, never a hang).
pub async fn retry_until_ok_wake<T, E>(
    timeout: Duration,
    max_wait: Duration,
    mut wake: impl FnMut() -> futures::future::BoxFuture<'static, ()>,
    mut attempt: impl AsyncFnMut() -> Result<T, E>,
) -> Result<T, E> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match attempt().await {
            Ok(value) => return Ok(value),
            Err(e) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(e);
                }
                let _ = tokio::time::timeout(max_wait, wake()).await;
            }
        }
    }
}
