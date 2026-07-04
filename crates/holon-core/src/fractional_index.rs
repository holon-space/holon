//! Fractional index utilities for block ordering
//!
//! This module provides utilities for generating fractional index keys
//! used for maintaining block order in hierarchical structures.
//!
//! # Why `loro_fractional_index` is allowed in `holon-core`
//!
//! `loro_fractional_index` is a **pure-math utility crate** (fractional
//! indexing over byte strings) published from the Loro workspace — it is NOT
//! the `loro` CRDT crate and pulls in no storage, no documents, no sync.
//! Depending on it here keeps the sort-key math identical between the Loro
//! order-owner and the SqlOnly order-owner, which is exactly the invariant
//! ADR 0005 needs. Reviewed and deliberately KEPT during the storage de-leak
//! pass (plan quirky-dazzling-map, Stage 8) — do not "clean this up" into a
//! hand-rolled reimplementation.

use anyhow::Context;
use anyhow::Result;
use loro_fractional_index::FractionalIndex;

/// Maximum length for sort_key before triggering rebalancing
pub const MAX_SORT_KEY_LENGTH: usize = 32;

/// Default fractional-index key for a not-yet-positioned block, matching the
/// SQL column default. Upper-case so lexicographic order agrees with
/// `FractionalIndex::to_string()` output (which uses upper-case hex). This is
/// the single owner of the default value — the SqlOnly order owner mints from
/// it and the `block` table column defaults to it; it is the adapter's
/// internal ordering encoding and never appears on the domain `Block` (ADR
/// 0005).
pub const DEFAULT_SORT_KEY: &str = "A0";

/// The [`DEFAULT_SORT_KEY`] as an owned `String`.
pub fn default_sort_key() -> String {
    DEFAULT_SORT_KEY.to_string()
}

/// The fractional-index terminator byte, rendered as this module's hex
/// encoding. Every key the generators below emit ends with it — that is what
/// makes a key a valid operand of [`gen_key_between`].
const TERMINATOR_HEX: &str = "80";

/// True when `key` is a well-formed fractional index — a value some generator
/// in this module actually produced: an even-length UPPER-CASE hex string of at
/// least one byte whose final byte is the terminator.
///
/// The case requirement is not cosmetic. `FractionalIndex::to_string` emits
/// `{:02X}`, so every minted key is upper-case, and `sort_key` is compared
/// lexicographically as text. A lower-case value ordered against upper-case
/// ones puts the column's text order at odds with the underlying byte order
/// (`'a' > 'F'`), so accepting one as a position would reorder siblings. Such a
/// value is hand-written vault data, and the correct story for it is the
/// unkeyed → re-mint path below.
///
/// Anything else is **not** a position in the keyspace, only a string that
/// happens to live in the `sort_key` column: [`DEFAULT_SORT_KEY`] (`"A0"` — a
/// single unterminated byte, the SQL column default meaning "never
/// positioned") and legacy hand-written values. Feeding one to
/// [`gen_key_between`] cannot work: for two equal-length operands the
/// generator only searches the bytes *before* the last one, so an unterminated
/// key of the same length as its neighbour yields no key at all, and a value
/// shorter than one byte makes it panic. Order owners must therefore classify
/// with this predicate and re-mint the unkeyed rows rather than treating their
/// column value as a position.
pub fn is_minted_key(key: &str) -> bool {
    key.len() >= 2
        && key.len().is_multiple_of(2)
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase() && b.is_ascii_hexdigit())
        && key.ends_with(TERMINATOR_HEX)
}

/// Generate a fractional index between two optional keys
///
/// # Arguments
/// * `prev_key` - The sort_key of the predecessor block (None if inserting at
///   beginning)
/// * `next_key` - The sort_key of the successor block (None if inserting at
///   end)
///
/// # Returns
/// A new sort_key that sorts between prev_key and next_key
///
/// Both bounds must satisfy [`is_minted_key`]. A caller holding a column value
/// it has not classified is holding a string, not a position — passing one
/// here is the caller's bug and is reported as such, naming the value.
pub fn gen_key_between(prev_key: Option<&str>, next_key: Option<&str>) -> Result<String> {
    for (side, key) in [("prev", prev_key), ("next", next_key)] {
        let Some(key) = key else { continue };
        if !is_minted_key(key) {
            anyhow::bail!(
                "{side} bound {key:?} is not a minted fractional index (the SQL default {DEFAULT_SORT_KEY:?} \
                 and legacy values are unkeyed sentinels, not positions) — the order owner must \
                 re-mint the row instead of anchoring on its column value"
            );
        }
    }

    let prev_index = prev_key.map(FractionalIndex::from_hex_string);

    let next_index = next_key.map(FractionalIndex::from_hex_string);

    let new_index = FractionalIndex::new(prev_index.as_ref(), next_index.as_ref()).with_context(
        || {
            format!(
                "Failed to generate fractional index between given keys ({prev_key:?}, {next_key:?})"
            )
        },
    )?;

    Ok(new_index.to_string())
}

/// Generate N evenly-spaced fractional index keys
///
/// Used for rebalancing siblings to create uniform spacing.
///
/// # Arguments
/// * `count` - Total number of keys to generate
///
/// # Returns
/// A vector of evenly-spaced sort_keys
pub fn gen_n_keys(count: usize) -> Result<Vec<String>> {
    let indices = FractionalIndex::generate_n_evenly(None, None, count)
        .context("Failed to generate evenly-spaced fractional indices")?;

    Ok(indices.into_iter().map(|idx| idx.to_string()).collect())
}

/// Generate a fractional index that comes after the given key
///
/// This function generates a key that sorts immediately after the given key.
/// Conceptually uses the same approach as `FractionalIndex::new_after()` (which
/// is pub(crate) and not directly accessible), but achieves the same result
/// by using `gen_key_between` with no upper bound.
///
/// # Arguments
/// * `prev_key` - The sort_key to come after (hex string format)
///
/// # Returns
/// A new sort_key that sorts after prev_key
pub fn gen_key_after(prev_key: &str) -> Result<String> {
    gen_key_between(Some(prev_key), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gen_key_between_empty() {
        let key = gen_key_between(None, None).unwrap();
        assert!(!key.is_empty());
    }

    #[test]
    fn test_gen_key_between_at_beginning() {
        let next = gen_key_between(None, None).unwrap();
        let new_key = gen_key_between(None, Some(&next)).unwrap();

        assert!(new_key < next);
    }

    #[test]
    fn test_gen_key_between_at_end() {
        let prev = gen_key_between(None, None).unwrap();
        let new_key = gen_key_between(Some(&prev), None).unwrap();

        assert!(new_key > prev);
    }

    #[test]
    fn test_gen_key_between_middle() {
        let first = gen_key_between(None, None).unwrap();
        let third = gen_key_between(Some(&first), None).unwrap();
        let second = gen_key_between(Some(&first), Some(&third)).unwrap();

        assert!(first < second);
        assert!(second < third);
    }

    #[test]
    fn test_gen_key_ordering() {
        let mut keys = Vec::new();
        let mut prev: Option<String> = None;

        for _ in 0..10 {
            let key = gen_key_between(prev.as_deref(), None).unwrap();
            keys.push(key.clone());
            prev = Some(key);
        }

        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
    }

    /// Undo of a structural move RE-MINTS the order key rather than restoring
    /// the captured one (`undo::is_derived_positional_field`), so the contract
    /// is sibling ORDER and the bytes are free. Whether they nevertheless come
    /// back identical depends only on the block's sibling position — which is
    /// why one observer reports "undo restored sort_key exactly" and the next
    /// reports it corrupted, both correctly.
    ///
    /// The keys below are the ones an append-only ingest of three children
    /// actually produces, so both arms quote values an observer really sees.
    #[test]
    fn undo_remints_the_order_key_and_only_order_is_guaranteed() {
        let a = gen_key_between(None, None).unwrap();
        let b = gen_key_between(Some(&a), None).unwrap();
        let c = gen_key_between(Some(&b), None).unwrap();
        assert_eq!((a.as_str(), b.as_str(), c.as_str()), ("80", "8180", "8280"));

        // LAST child: undo re-places it after `a` with no successor. The mint is
        // idempotent, so the key returns byte-identical and nothing looks amiss.
        assert_eq!(gen_key_between(Some(&a), None).unwrap(), b);

        // MIDDLE child: undo re-places it between `a` and the successor that
        // never moved. The mint CANNOT reproduce `b` — it must land strictly
        // between two keys that already bracket it — yet the order is exact.
        let remint = gen_key_between(Some(&a), Some(&c)).unwrap();
        assert_eq!(remint, "827F80");
        assert_ne!(remint, b);
        assert!(
            a < remint && remint < c,
            "order is the contract, not the bytes"
        );
    }

    #[test]
    fn test_gen_n_keys() {
        let keys = gen_n_keys(5).unwrap();
        assert_eq!(keys.len(), 5);

        for i in 0..keys.len() - 1 {
            assert!(keys[i] < keys[i + 1]);
        }
    }

    #[test]
    fn test_gen_n_keys_empty() {
        let keys = gen_n_keys(0).unwrap();
        assert_eq!(keys.len(), 0);
    }

    #[test]
    fn test_gen_n_keys_single() {
        let keys = gen_n_keys(1).unwrap();
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn test_deep_nesting() {
        let mut keys = vec![gen_key_between(None, None).unwrap()];

        for _ in 0..50 {
            let last = keys.last().unwrap();
            let new_key = gen_key_between(Some(last), None).unwrap();
            keys.push(new_key);
        }

        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);

        let max_len = keys.iter().map(|k| k.len()).max().unwrap();
        println!("Max key length after 50 insertions: {}", max_len);
        assert!(max_len < MAX_SORT_KEY_LENGTH);
    }

    #[test]
    fn test_rebalancing() {
        let count = 10;
        let keys = gen_n_keys(count).unwrap();

        assert_eq!(keys.len(), count);

        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);

        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), keys.len());

        let max_len = keys.iter().map(|k| k.len()).max().unwrap();
        println!("Max key length for {} keys: {}", count, max_len);
        assert!(max_len < MAX_SORT_KEY_LENGTH);
    }

    #[test]
    fn test_default_sort_key_matches_sql_column_default() {
        // The SQL `block.sort_key` column default is the literal "A0";
        // both must stay in lockstep (this fn is the single owner).
        assert_eq!(default_sort_key(), "A0");
        assert_eq!(default_sort_key(), DEFAULT_SORT_KEY);
    }

    /// `"A0"` is a single *unterminated* byte, so it is not a position in the
    /// keyspace at all — it only means "this row was never positioned".
    /// Anchoring on it is what made `split_block` fail in SqlOnly mode.
    #[test]
    fn the_sql_default_is_not_a_position_and_cannot_be_anchored_on() {
        assert!(!is_minted_key(DEFAULT_SORT_KEY));

        let err = format!("{:#}", gen_key_after(DEFAULT_SORT_KEY).unwrap_err());
        assert!(err.contains("not a minted fractional index"), "{err}");
        assert!(err.contains("A0"), "{err}");
    }

    /// The defect's exact arithmetic: the generator only searches the bytes
    /// *before* the last one, so for two equal-length operands an unterminated
    /// key can never be separated from its neighbour. Both single-byte keys —
    /// the real `"80"` and the sentinel `"A0"` — hit this, which is the pair
    /// `new_child_anchor` produced when one sibling still carried the column
    /// default.
    #[test]
    fn a_single_byte_pair_has_no_key_between_it() {
        let real = gen_key_between(None, None).unwrap();
        assert_eq!(real, "80");
        assert!(real.as_str() < DEFAULT_SORT_KEY);

        let err = format!(
            "{:#}",
            gen_key_between(Some(&real), Some(DEFAULT_SORT_KEY)).unwrap_err()
        );
        assert!(err.contains("not a minted fractional index"), "{err}");
    }

    /// The predicate's job: separate minted positions from every other string
    /// that can reach the `sort_key` column.
    #[test]
    fn is_minted_key_accepts_only_generator_output() {
        for key in gen_n_keys(7).unwrap() {
            assert!(is_minted_key(&key), "{key}");
            assert!(is_minted_key(&gen_key_after(&key).unwrap()), "{key}");
        }
        for junk in [
            "",     // never-written column
            "A0",   // the SQL default sentinel
            "V",    // legacy hand-written value
            "8",    // half a byte
            "a0",   // lower-case, and unterminated
            "ZZ80", // not hex
            "8081", // hex, but the terminator is not last
            "a080", // lower-case: text order would fight byte order
            "ff80", // same, and it would sort above every minted key
            "aB80", // mixed case
        ] {
            assert!(!is_minted_key(junk), "{junk:?} must not pass");
        }
    }

    /// Why the predicate is case-sensitive: `sort_key` is compared as TEXT, so
    /// a lower-case value accepted as a position would order against minted
    /// (upper-case) keys by ASCII, not by the bytes it encodes. Here `"a080"`
    /// and `"A080"` denote the SAME index, yet the lower-case spelling sorts
    /// above a strictly greater minted key.
    #[test]
    fn lower_case_would_invert_the_order_it_encodes() {
        let low = "a080";
        let high = gen_key_after("A080").unwrap();

        assert_eq!(
            loro_fractional_index::FractionalIndex::from_hex_string(low),
            loro_fractional_index::FractionalIndex::from_hex_string("A080"),
            "the two spellings encode the same index"
        );
        assert!(high.as_str() > "A080", "{high} must follow A080");
        assert!(
            low > high.as_str(),
            "text order puts {low} above its own successor {high} — exactly why it is not a \
             position"
        );
        assert!(!is_minted_key(low));
    }

    /// The contract the order owners rely on: any two minted keys always have
    /// a minted key strictly between them. This is what makes "classify, then
    /// only ever anchor on minted keys" a total strategy rather than a
    /// narrower failure window.
    #[test]
    fn any_two_minted_keys_admit_a_key_between_them() {
        let mut keys = gen_n_keys(9).unwrap();
        for _ in 0..40 {
            let mut next = Vec::with_capacity(keys.len() * 2 - 1);
            for pair in keys.windows(2) {
                let mid = gen_key_between(Some(&pair[0]), Some(&pair[1]))
                    .unwrap_or_else(|e| panic!("between {:?} and {:?}: {e:#}", pair[0], pair[1]));
                assert!(
                    pair[0] < mid && mid < pair[1],
                    "{:?} {mid} {:?}",
                    pair[0],
                    pair[1]
                );
                assert!(is_minted_key(&mid), "{mid}");
                next.push(pair[0].clone());
                next.push(mid);
            }
            next.push(keys.last().unwrap().clone());
            keys = next;
            keys.truncate(9);
        }
    }

    #[test]
    fn test_gen_key_after() {
        let prev_key = gen_key_between(None, None).unwrap();
        let after_key = gen_key_after(&prev_key).unwrap();

        assert!(after_key > prev_key);
        // The generated key must itself be a valid hex fractional index
        // (parse round-trip), not just any lexicographically-larger string.
        assert_eq!(
            loro_fractional_index::FractionalIndex::from_hex_string(&after_key).to_string(),
            after_key
        );

        let mut keys = vec![gen_key_between(None, None).unwrap()];
        for _ in 0..5 {
            let last = keys.last().unwrap();
            let next = gen_key_after(last).unwrap();
            keys.push(next);
        }

        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted);
        // Strictly increasing: a constant key would survive the sort check.
        for pair in keys.windows(2) {
            assert!(
                pair[0] < pair[1],
                "keys must be strictly increasing: {pair:?}"
            );
        }
    }
}
