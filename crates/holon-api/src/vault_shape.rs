//! Anonymized structural profile of a vault.
//!
//! A `VaultShapeProfile` captures the SHAPE of a real vault — counts and
//! distributions only, ZERO strings lifted from the vault (no page names, no
//! content, no ids, no paths). It is the environment-parity input that lets the
//! keystone + windowed PBT generators widen their generated vaults toward the
//! complexity real vaults actually reach.
//!
//! All maps are `BTreeMap`, so `serde_json::to_string_pretty` emits sorted keys
//! and the profile is byte-stable across runs (deterministic output).

use std::collections::BTreeMap;

use serde::Deserialize;
use serde::Serialize;

/// Bump on any breaking schema change. Consumers assert on this.
pub const VAULT_SHAPE_SCHEMA_VERSION: u32 = 1;

/// A distribution over small non-negative integers (value -> occurrence count).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Histogram {
    /// value -> count. `BTreeMap` gives deterministic, sorted key order.
    pub counts: BTreeMap<u32, u64>,
}

impl Histogram {
    pub fn record(&mut self, value: u32) {
        *self.counts.entry(value).or_insert(0) += 1;
    }

    pub fn total(&self) -> u64 {
        self.counts.values().copied().sum()
    }

    /// The largest observed value (0 when empty).
    pub fn max_value(&self) -> u32 {
        self.counts.keys().copied().next_back().unwrap_or(0)
    }

    /// The value at the given percentile in `0.0..=1.0` (0 when empty). Uses
    /// the nearest-rank method on the cumulative count — deterministic.
    /// `percentile(1.0) == max_value()`.
    pub fn percentile(&self, p: f64) -> u32 {
        let total = self.total();
        if total == 0 {
            return 0;
        }
        let rank = (p.clamp(0.0, 1.0) * total as f64).ceil().max(1.0) as u64;
        let mut cumulative = 0u64;
        for (value, count) in &self.counts {
            cumulative += count;
            if cumulative >= rank {
                return *value;
            }
        }
        self.max_value()
    }
}

/// A distribution over BUCKETED content lengths (bucket-lower-bound -> count).
/// Bucketing keeps the profile coarse (privacy: no exact per-block lengths
/// leak, only band counts) and stable.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct BucketHistogram {
    /// bucket lower bound -> count.
    pub buckets: BTreeMap<u32, u64>,
}

/// Content-length bucket lower bounds (chars). A length lands in the largest
/// bound `<=` it.
pub const CONTENT_LENGTH_BUCKETS: &[u32] = &[0, 8, 16, 32, 64, 128, 256, 512, 1024];

impl BucketHistogram {
    pub fn record(&mut self, length: u32) {
        let bound = CONTENT_LENGTH_BUCKETS
            .iter()
            .rev()
            .find(|&&b| b <= length)
            .copied()
            .unwrap_or(0);
        *self.buckets.entry(bound).or_insert(0) += 1;
    }

    pub fn total(&self) -> u64 {
        self.buckets.values().copied().sum()
    }

    /// Lower bound of the bucket at the given percentile (0 when empty).
    pub fn percentile_bound(&self, p: f64) -> u32 {
        let total = self.total();
        if total == 0 {
            return 0;
        }
        let rank = (p.clamp(0.0, 1.0) * total as f64).ceil().max(1.0) as u64;
        let mut cumulative = 0u64;
        for (bound, count) in &self.buckets {
            cumulative += count;
            if cumulative >= rank {
                return *bound;
            }
        }
        self.buckets.keys().copied().next_back().unwrap_or(0)
    }
}

/// Structural statistics for one vault. Counts and distributions ONLY.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct VaultShapeProfile {
    pub schema_version: u32,
    /// Total `.org` files walked.
    pub file_count: u64,
    /// Directories that have a same-name `.org` companion beside them.
    pub companion_pair_count: u64,
    /// Fraction of files carrying no `#+ID:` document id (`0.0..=1.0`).
    pub idless_file_ratio: f64,
    /// Fraction of headline blocks carrying no `:ID:` property (`0.0..=1.0`).
    pub idless_block_ratio: f64,
    /// Blocks per file.
    pub blocks_per_file: Histogram,
    /// Nesting depth of each block (document root = 0).
    pub depth: Histogram,
    /// Child count of each parent that has children.
    pub sibling_count: Histogram,
    /// Bucketed content character length per block.
    pub content_length: BucketHistogram,
    /// Inline `Link`-mark count per block.
    pub links_per_block: Histogram,
    /// Total tag occurrences across all blocks (excludes the `Page` marker).
    pub tag_usage: u64,
    /// Distinct tag COUNT (a number, never the tag strings).
    pub distinct_tag_count: u64,
    /// Total property occurrences across all blocks (excludes `ID`).
    pub property_usage: u64,
    /// Distinct property-key COUNT (a number, never the keys).
    pub distinct_property_count: u64,
}

impl VaultShapeProfile {
    /// Serialize deterministically (sorted keys, trailing newline).
    pub fn to_json(&self) -> String {
        let mut s = serde_json::to_string_pretty(self).expect("profile serialization is total");
        s.push('\n');
        s
    }

    // ─── Generator accessors ─────────────────────────────────────────────
    // These derive proptest range UPPER BOUNDS from the distributions. The
    // generator uses them to build plain ranges/regex, so shrinking still
    // reduces toward small regardless of the profile.

    /// Upper bound (>=1) for the per-file block-count range. p95 so a few huge
    /// files don't blow the generated case size, floored at 1.
    pub fn blocks_per_file_bound(&self) -> u32 {
        self.blocks_per_file.percentile(0.95).max(1)
    }

    /// Max nesting depth to generate (>=1). Real depth widens the parent-linked
    /// tree the flat default never builds.
    pub fn depth_bound(&self) -> u32 {
        self.depth.percentile(0.95).max(1)
    }

    /// Upper bound (>=1) for generated content character length.
    pub fn content_length_bound(&self) -> u32 {
        self.content_length.percentile_bound(0.95).max(1)
    }
}
