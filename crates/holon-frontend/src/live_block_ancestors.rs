//! The `live_block` cycle rule.
//!
//! An `A → B → A` embed must not mount a third `A`. How the chain reaches a
//! nested block is per-frontend plumbing — GPUI stores it on the
//! `ReactiveShell` at creation time, the web provides it down the Dioxus
//! context — but the rule itself is one comparison and belongs in one place,
//! because a frontend that forgets it does not render a bounded-depth cycle:
//! it opens an unbounded chain of live subscriptions.

/// Chain of `live_block` block ids being rendered down the view tree.
///
/// Equality on the contained ids is canonical-string equality — the same ids
/// that flow into a frontend's per-block cache key. The chain is cheap to
/// extend (one `Vec<String>` clone, typically <= 4 entries).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveBlockAncestors {
    inner: Vec<String>,
}

impl LiveBlockAncestors {
    pub fn new() -> Self {
        Self::default()
    }

    /// Would mounting `id` here close a cycle?
    pub fn would_cycle(&self, id: &str) -> bool {
        self.inner.iter().any(|x| x == id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.would_cycle(id)
    }

    /// Return a new chain with `id` appended. The receiver is unchanged so
    /// callers can keep using the parent chain after spawning a child.
    pub fn pushed(&self, id: impl Into<String>) -> Self {
        let mut c = self.inner.clone();
        c.push(id.into());
        Self { inner: c }
    }

    pub fn as_slice(&self) -> &[String] {
        &self.inner
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pushed_is_an_immutable_copy() {
        let a = LiveBlockAncestors::new();
        assert!(a.is_empty());
        let b = a.pushed("block:A");
        assert!(a.is_empty(), "parent chain stays unchanged");
        assert!(b.contains("block:A"));
        let c = b.pushed("block:B");
        assert!(c.contains("block:A"));
        assert!(c.contains("block:B"));
        assert!(!c.contains("block:C"));
    }

    /// The rule the web arm had no counterpart for: re-entering a block
    /// already on the chain is a cycle, at any depth.
    #[test]
    fn would_cycle_detects_reentry_at_any_depth() {
        let chain = LiveBlockAncestors::new()
            .pushed("block:A")
            .pushed("block:B")
            .pushed("block:C");
        assert!(chain.would_cycle("block:A"));
        assert!(chain.would_cycle("block:C"));
        assert!(!chain.would_cycle("block:D"));
    }
}
