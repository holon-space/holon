//! Framework-agnostic data-sync echo/convergence policy.
//!
//! Moved out of the GPUI `EditorView` so the echo/converge decision is owned by
//! the frontend-agnostic layer (see `EditorViewModel`) and is exercisable by
//! the headless keystone PBT. Pure and side-effect-free — the convergence
//! policy is unit-tested directly (see the `echo_suppression` tests below)
//! without a live gpui window.

/// Outcome of applying the op-versioned echo-suppression rule to one data-sync
/// emission. Pure and side-effect-free so the convergence policy is unit-tested
/// directly (see the `echo_suppression` tests) without a live gpui window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoDecision {
    /// Echo equals the editor's current InputState — nothing to change. If the
    /// echo carried a sequence, advance the high-water mark to it so a later
    /// reordered echo of an even earlier keystroke is still recognised as
    /// stale.
    InSync { advance_to: Option<i64> },
    /// Converge InputState to the echo and adopt `seq` as the new high-water.
    Converge { seq: i64 },
    /// A reordered/lagged echo of an edit strictly older than the editor's last
    /// local write. Drop it — this is the "typing resets the block" fix.
    DropStale,
    /// The echo carries the editor's OWN last write-seq AND its content is
    /// exactly the trailing-whitespace canonicalization of the focused buffer
    /// (`SqlOperationProvider::trimmed_content` strips trailing whitespace on
    /// store). This is the SQL-canonicalized echo of the user's own in-flight
    /// write, NOT a newer external authority — the two are indistinguishable by
    /// seq alone because non-editor writers don't bump `write_seq`. Converging
    /// would delete the just-typed trailing space from the focused buffer (the
    /// "typed space vanishes ~100ms later" bug). Keep the visible buffer as
    /// typed; adopt the canonical text as the change-tracking baseline so a
    /// later blur/idle diffs against SQL truth, not a stale baseline.
    AdoptBaseline { seq: i64 },
    /// Content changed but the row carried no `write_seq` ordering token — a
    /// schema/projection regression. Drop and report loudly (never converge
    /// blindly: that is the stale-echo data loss we are preventing).
    DropNoSeq,
}

/// Op-versioned echo suppression for the SqlOnly data-sync path.
///
/// Converge to an authority state only when it is **at least as new** as the
/// editor's last local write (`echo_seq >= last_local_seq`). A stale/reordered
/// echo of an earlier keystroke (`echo_seq < last_local_seq`) is dropped; a
/// `split_block` truncation or peer edit issued after the last keystroke
/// carries a greater-or-equal seq and still converges. Content equality
/// short-circuits (the editor's own latest echo, or a redundant emit). Ordering
/// — not content — is authoritative because the dispatcher's inline-mark
/// stripping rewrites the stored value, so an editor's own echo legitimately
/// differs from what it typed.
pub fn evaluate_data_sync_echo(
    current: &str,
    new_value: &str,
    echo_seq: Option<i64>,
    last_local_seq: i64,
) -> EchoDecision {
    if current == new_value {
        return EchoDecision::InSync {
            advance_to: echo_seq,
        };
    }
    let Some(seq) = echo_seq else {
        return EchoDecision::DropNoSeq;
    };
    if seq < last_local_seq {
        EchoDecision::DropStale
    } else if seq == last_local_seq
        && holon_api::content_canonical::canonicalize_stored_content(current, false) == new_value
    {
        // Same seq as our last local write AND the echo is exactly the SERVER
        // canonicalization of the focused buffer. Non-editor writers
        // (split/join/org) do NOT bump `write_seq`, so a genuine structural write
        // echoes at `seq == last_local` too — but its content is a substantive
        // change, never merely the whitespace-canonicalization of `current`. This
        // branch is therefore the SQL-canonicalized echo of the user's OWN
        // in-flight write. Converging would delete the just-typed whitespace (the
        // "typed space vanishes ~100ms later" bug, whether at the end of the
        // buffer or at the end of a multiline block's first line). Keep the
        // buffer; the caller adopts the canonical baseline instead.
        //
        // The canonicalization MUST match what the store actually applied to
        // produce this echo, so it delegates to the single shared definition
        // (`holon_api::content_canonical`) that `SqlOperationProvider::trimmed_content`
        // also calls — the two can never drift. `is_source = false` is a
        // deliberate approximation: the provider resolves `is_source` from the
        // block's REAL content_type via a live DB lookup
        // (`SqlOperationProvider`, set_field paths ~:2152-2175 / ~:1636-1654),
        // which this pure function cannot see. For text blocks both sides use
        // the text rule (exact match). For SOURCE blocks the store applies only
        // `trim_end`, so a pure-trailing trim still matches (the reported bug
        // stays fixed) and a first-line/leading divergence falls through to
        // Converge — the pre-fix behavior, disclosed gap, no new data loss.
        EchoDecision::AdoptBaseline { seq }
    } else {
        // `seq > last_local` (a genuinely newer authority — a peer edit stamps a
        // fresh seq), OR `seq == last_local` with a substantive external change
        // (a split truncation "hello world" -> "hello" shares the editor's seq
        // but is not a trailing-whitespace trim of the buffer). Converge.
        EchoDecision::Converge { seq }
    }
}

#[cfg(test)]
mod echo_suppression {
    //! Directed regression tests for the op-versioned data-sync echo guard —
    //! the fix for the vault-scale P1 "typing `[[` (or any edit) resets the
    //! whole block to its pre-typing content".
    //!
    //! The failure is a focused editor converging to a STALE/reordered CDC echo
    //! of an earlier keystroke. These exercise the pure decision function
    //! [`evaluate_data_sync_echo`] the data-sync closure delegates to,
    //! modelling an INJECTED-DELAY, in-flight-typing timeline
    //! deterministically (no gpui window, no real latency needed).
    //!
    //! RED-FIRST equivalence: the old policy converged whenever the editor was
    //! "idle" (`prev_synced == current`), which is true the instant a keystroke
    //! settles — so `stale_echo_while_typing_ahead_is_dropped` below would have
    //! CONVERGED (reset the block) under the old code. The seq guard makes it a
    //! drop.

    use super::EchoDecision;
    use super::evaluate_data_sync_echo;

    // A block seeded at boot carries write_seq 0 (the column default) until the
    // editor writes it. The editor's own keystrokes carry strictly-increasing
    // process-global sequences (holon_api::write_seq::next()).
    const SEED: i64 = 0;

    #[test]
    fn stale_echo_while_typing_ahead_is_dropped() {
        // Timeline: user typed "ab" (seq 10) then "abc" (seq 11); InputState is
        // now "abc" and last_local_seq is 11. The CDC echo of the EARLIER "ab"
        // write (seq 10) arrives late. It must be DROPPED — converging would
        // reset the visible text backwards to "ab". This is the exact P1.
        let d = evaluate_data_sync_echo("abc", "ab", Some(10), 11);
        assert_eq!(d, EchoDecision::DropStale);
    }

    #[test]
    fn pre_typing_stale_echo_is_dropped() {
        // The reported symptom: block content is "Block 07-010 ..." pre-typing.
        // The user types, advancing last_local_seq to 600. A lagged echo of the
        // pre-typing content (an older, smaller seq) must not resurrect it.
        let d = evaluate_data_sync_echo(
            "Block 07-010 ...hello",
            "Block 07-010 ...", // pre-typing content
            Some(305),
            600,
        );
        assert_eq!(d, EchoDecision::DropStale);
    }

    #[test]
    fn split_truncation_after_last_keystroke_still_converges() {
        // A split_block issued AFTER the last keystroke gets a greater seq, so
        // the surviving (reused) editor still converges to the truncated content
        // while it owns focus — the property the old idle-heuristic preserved and
        // the seq guard must keep.
        let d = evaluate_data_sync_echo("hello world", "hello", Some(12), 11);
        assert_eq!(d, EchoDecision::Converge { seq: 12 });
    }

    #[test]
    fn equal_seq_external_write_converges() {
        // Non-editor writers (split/join/org) don't bump write_seq, so the row
        // retains the editor's last seq; their echo carries seq == last_local and
        // a DIFFERENT value → converge (they changed content, not the token).
        // The truncation "hello world" -> "hello" is NOT a trailing-whitespace
        // canonicalization of the buffer (`"hello world".trim_end()` != "hello"),
        // so it is a genuine external change and still converges even though it
        // shares the editor's seq. This is the discriminator that keeps the
        // trailing-space fix from swallowing real structural writes.
        let d = evaluate_data_sync_echo("hello world", "hello", Some(11), 11);
        assert_eq!(d, EchoDecision::Converge { seq: 11 });
    }

    #[test]
    fn same_seq_trailing_space_echo_adopts_baseline_not_converge() {
        // THE TRAILING-SPACE BUG. The user typed "foo " (trailing space); the
        // editor stamped write_seq 11 on that content write. The SQL provider
        // trims trailing whitespace on store ("foo") and echoes it back through
        // CDC carrying the SAME write_seq 11 (the trim does not re-stamp). At the
        // moment the echo arrives the focused buffer is still "foo " and
        // last_local_seq is 11. This echo is the SQL-canonicalized form of the
        // user's OWN in-flight write, distinguished from a genuine same-seq
        // external write by `"foo ".trim_end() == "foo"`. Converging would delete
        // the just-typed space; the correct decision keeps the buffer and adopts
        // the canonical baseline.
        let d = evaluate_data_sync_echo("foo ", "foo", Some(11), 11);
        assert_eq!(d, EchoDecision::AdoptBaseline { seq: 11 });
    }

    #[test]
    fn same_seq_multiple_trailing_spaces_echo_adopts_baseline() {
        // Multiple trailing spaces collapse to the same canonical form, so the
        // discriminator (`trim_end`) still recognises the echo as our own write.
        let d = evaluate_data_sync_echo("hello world   ", "hello world", Some(42), 42);
        assert_eq!(d, EchoDecision::AdoptBaseline { seq: 42 });
    }

    #[test]
    fn same_seq_first_line_trailing_space_in_multiline_adopts_baseline() {
        // CLASS EXTENSION. The server ALSO trims the FIRST line's trailing
        // whitespace in multiline text content (the first line becomes the org
        // headline). A space typed at the end of a multiline block's first line
        // echoes back canonicalized ("foo \nbar" -> "foo\nbar") at the SAME seq.
        // `current.trim_end()` cannot see this — the stripped space is interior
        // to the whole string — so the discriminator must use the FULL server
        // canonicalization (`holon_api::content_canonical`). Without it this
        // narrower instance of the same bug still eats the space.
        let d = evaluate_data_sync_echo("foo \nbar", "foo\nbar", Some(11), 11);
        assert_eq!(d, EchoDecision::AdoptBaseline { seq: 11 });
    }

    #[test]
    fn blur_reecho_after_adopt_does_not_loop() {
        // BLUR-AFTER-ADOPT LIVENESS. After AdoptBaseline the buffer ("foo ") and
        // SQL truth ("foo") diverge by a trailing space while focused. On blur
        // the change-tracking (baselined to "foo") diffs the live "foo " and
        // re-dispatches ONE set_field("foo ") — a blur intent carries NO
        // write_seq, so the provider trims to "foo" and leaves the write_seq
        // column at the editor's last value N. The re-echo is therefore
        // ("foo", seq N) at last_local N. Two terminal buffer states are
        // possible when that re-echo lands, and NEITHER re-converges — so the
        // "own canonicalized echo" cannot become an infinite Converge loop:
        //
        //   (1) the unfocused render backstop has already canonicalized the
        //       buffer to "foo": the re-echo equals the buffer → InSync.
        let settled = evaluate_data_sync_echo("foo", "foo", Some(600), 600);
        assert_eq!(
            settled,
            EchoDecision::InSync {
                advance_to: Some(600)
            }
        );
        //   (2) the backstop has not run yet and the buffer is still "foo ":
        //       the re-echo is again the trailing-whitespace canonicalization →
        //       AdoptBaseline (a no-op re-baseline, still no set_value).
        let still_focused = evaluate_data_sync_echo("foo ", "foo", Some(600), 600);
        assert_eq!(still_focused, EchoDecision::AdoptBaseline { seq: 600 });
        // The trim is idempotent server-side, so a single benign blur
        // re-dispatch is acceptable; the absence of any Converge here is what
        // rules out the pathological echo loop.
        assert!(!matches!(settled, EchoDecision::Converge { .. }));
        assert!(!matches!(still_focused, EchoDecision::Converge { .. }));
    }

    #[test]
    fn self_echo_is_in_sync_and_advances_high_water() {
        // The confirming echo of our own latest write equals current InputState.
        let d = evaluate_data_sync_echo("abc", "abc", Some(11), 11);
        assert_eq!(
            d,
            EchoDecision::InSync {
                advance_to: Some(11)
            }
        );
    }

    #[test]
    fn pre_typing_editor_converges_to_external_seed() {
        // Before the user types (last_local_seq == SEED == 0) every external
        // state is at least as new → converge. This is correct seeding: a
        // freshly focused editor adopts the authority content.
        let d = evaluate_data_sync_echo("stale", "fresh from peer", Some(1), SEED);
        assert_eq!(d, EchoDecision::Converge { seq: 1 });
    }

    #[test]
    fn missing_seq_on_changed_content_fails_loud_and_drops() {
        // A content change with no write_seq token is a schema/projection
        // regression: drop (never converge blindly) — the loud tracing::error!
        // lives at the call site.
        let d = evaluate_data_sync_echo("abc", "different", None, 11);
        assert_eq!(d, EchoDecision::DropNoSeq);
    }

    #[test]
    fn missing_seq_but_in_sync_is_noop_without_advance() {
        // No token, but the echo equals current — a benign redundant emit. In
        // sync, and there is no seq to advance the high-water mark to.
        let d = evaluate_data_sync_echo("abc", "abc", None, 11);
        assert_eq!(d, EchoDecision::InSync { advance_to: None });
    }
}
