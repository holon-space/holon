//! The four axes a fixture list peer is parameterized over.
//!
//! A real remote-list API differs from the next along exactly these axes, and
//! everything the reconciler may know about a list is what its sidecar
//! declares. A fixture peer that varies along the same axes is therefore a
//! stand-in for a FAMILY of peers, not for one product. No name here is a
//! product's; the domain words are peer, list, row, key, watermark and batch.

/// How a row is identified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyShape {
    /// The peer issues the row id; identity is the `.id` expression.
    PeerIssuedId,
    /// Identity is content; the sidecar derives the key from a content pair.
    NaturalKey,
}

/// How a change is detected between rounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeDetection {
    /// The peer versions itself with a monotonic cursor; a commit must be based
    /// on the version the pull it followed returned, and a stale commit is a
    /// load-bearing failure, never a silent overwrite.
    VersionCursor,
    /// The peer serves a full snapshot each round and applies last-write-wins,
    /// ignoring the base version a batch carries.
    FullSnapshot,
}

/// How a commit applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitGranularity {
    /// Every command of a batch lands under one version bump.
    Batched,
    /// Each command lands under its own version bump.
    PerRow,
}

/// Whether the peer serves cached bodies and needs a bust knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    /// The peer serves the current list on every pull.
    Fresh,
    /// The transport serves the body it holds. Only a pull carrying the
    /// freshness argument the connection declares is answered from origin, so
    /// a connection that declares none is served a body older than a write the
    /// round knows landed — it reads that write as missing, and the round
    /// refuses rather than re-sending it.
    CachedNeedsBust,
}

/// One point in the four-axis space a fixture peer is parameterized over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixtureProfile {
    pub key_shape: KeyShape,
    pub change_detection: ChangeDetection,
    pub commit_granularity: CommitGranularity,
    pub cache_mode: CacheMode,
}
