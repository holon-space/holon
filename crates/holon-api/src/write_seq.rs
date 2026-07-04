//! Monotonic per-write ordering token for editor echo suppression.
//!
//! A [`WriteSeq`] is an **ordering-only** value: it says nothing about *what*
//! changed (that is [`crate::streaming::ChangeOrigin`]'s job) — only *when*, in
//! a total order across all writes issued by this process, a write happened.
//!
//! # Why it exists
//!
//! The gpui editor writes each keystroke as a `set_field("content")` and later
//! observes the change echoed back through the CDC → `LiveData` → row signal.
//! At vault scale the echo can arrive **late or reordered**: an echo carrying
//! an *earlier* keystroke's content lands after the user has typed further.
//! Without an ordering token the editor cannot tell "a genuinely newer
//! authority state" (a `split_block` truncation the user just triggered — must
//! converge) from "a stale echo of my own earlier keystroke" (must be dropped).
//! Comparing content strings cannot answer this (the dispatcher's inline-mark
//! stripping rewrites the stored value, so even the editor's own echo differs
//! from what it typed).
//!
//! The editor stamps each of its content writes with `WriteSeq::next()` and
//! records it as its last local sequence. On an echo it converges iff
//! `echo_seq >= last_local_seq` and drops otherwise. Because the counter is
//! process-global and monotonic, a `split_block` write issued *after* the last
//! keystroke gets a strictly greater sequence and therefore still converges,
//! while a reordered stale echo of an earlier keystroke gets a strictly smaller
//! one and is dropped — correct at any latency, with no timer.
//!
//! # Scope
//!
//! Ordering across writes *this process* issued. It is not a CRDT lamport clock
//! and carries no peer identity; cross-peer ordering remains Loro's concern.

use std::sync::atomic::AtomicI64;
use std::sync::atomic::Ordering;

/// A monotonic, process-global write-ordering token. Ordering-only (see module
/// docs). Wraps an `i64` because it round-trips through the
/// `block_raw.write_seq` SQL column (SQLite integers are `i64`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WriteSeq(i64);

impl WriteSeq {
    /// The sequence stamped on a row that has never been written by an editor
    /// (the SQL column default). Every real editor write is `> ZERO`, so an
    /// editor whose `last_local_seq` is `ZERO` converges to any echo — the
    /// correct pre-typing behaviour.
    pub const ZERO: WriteSeq = WriteSeq(0);

    /// The underlying integer, for SQL round-tripping only.
    pub fn get(self) -> i64 {
        self.0
    }

    /// Reconstruct from a value read back out of the `write_seq` column.
    pub fn from_i64(v: i64) -> WriteSeq {
        WriteSeq(v)
    }
}

/// Process-global counter. Starts at 0; the first `next()` returns 1, so any
/// editor-issued sequence is strictly greater than the `ZERO` default a
/// never-editor-written row carries.
static COUNTER: AtomicI64 = AtomicI64::new(0);

/// Allocate the next monotonic write sequence. Every editor content write calls
/// this exactly once and records the result as its last local sequence.
pub fn next() -> WriteSeq {
    WriteSeq(COUNTER.fetch_add(1, Ordering::SeqCst) + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_is_strictly_monotonic_and_above_zero() {
        let a = next();
        let b = next();
        let c = next();
        assert!(
            a > WriteSeq::ZERO,
            "first allocation must exceed the row default"
        );
        assert!(b > a && c > b, "sequences must be strictly increasing");
    }

    #[test]
    fn round_trips_through_i64() {
        let s = next();
        assert_eq!(WriteSeq::from_i64(s.get()), s);
    }
}
