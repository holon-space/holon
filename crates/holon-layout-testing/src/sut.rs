//! Capability traits + orphan-rule wrappers that let per-variant
//! `TransitionImpl` impls live in this crate yet be reusable across
//! frontends and across PBTs.
//!
//! ## The orphan rule problem
//!
//! `TransitionImpl<Ref, Sut>` is foreign (defined in `holon-pbt-core`)
//! and the variant structs (`SwitchViewMode`, …) are foreign too.
//! Writing `impl<S: Clickable> TransitionImpl<(), S> for SwitchViewMode`
//! in this crate would be all-foreign with only a type variable in the
//! `Sut` position → rejected by Rust's coherence rules.
//!
//! The fix is to insert a local type at the head of the `Sut` (and
//! `Ref`) positions: [`LayoutSut`] and [`LayoutRef`] are zero-cost
//! transparent wrappers whose only job is to satisfy the orphan rule.
//! Once a local concrete type appears in the trait-param sequence,
//! type variables in later positions are allowed.
//!
//! Frontends implement the small [`Clickable`] / [`LiveBlockSink`]
//! capability traits on their test session, then wrap it in `LayoutSut`
//! at the dispatch site. PBTs implement [`LayoutRefState`] on their
//! reference-state type and wrap in `LayoutRef`. The variant impls in
//! `crate::transitions::*` are shared by *both* the GPUI layout PBT and
//! the integration-tests PBT.

use std::marker::PhantomData;

/// Frontend capability: synthesize a real click at the centre of the
/// element registered under `element_id` in the frontend's bounds
/// registry. The variant impls call this primitive to drive any
/// transition that ultimately means "click a known widget".
pub trait Clickable {
    fn click_at_element(&mut self, element_id: &str);
}

/// Frontend capability: push deferred `live_block` content (the test-
/// harness side of async data arrival). Distinct from `Clickable`
/// because it's a test stimulus, not a user gesture.
pub trait LiveBlockSink {
    fn deliver_block_content_loaded(&mut self, block_id: &str);
}

/// PBT reference-state capability: expose the handle / mode / drawer
/// information the per-variant generators and preconditions need.
/// Both PBTs implement this on their own ref-state type so the same
/// `weighted_generator` body works for both.
pub trait LayoutRefState {
    /// VMS handles available for `SwitchViewMode` to target.
    fn switchable_handles(&self) -> &[crate::blueprint::BlockHandle];
    /// Drawer handles available for `ToggleDrawer` to target.
    fn drawer_handles(&self) -> &[crate::blueprint::DrawerHandle];
    /// Block ids registered with a deferred `live_block` placeholder
    /// that `DeliverBlockContent` may push content into.
    fn deferred_block_ids(&self) -> Vec<String>;
    /// Target ids of `expand_toggle` widgets currently rendered, that
    /// `ToggleCollapse` may target.
    fn collapsible_target_ids(&self) -> Vec<String> {
        Vec::new()
    }
    /// Currently-active view mode for `block_id`, if known.
    fn current_view_mode(&self, block_id: &str) -> Option<&str>;
    /// Is the drawer for `block_id` open right now?
    fn drawer_is_open(&self, block_id: &str) -> bool;
}

/// Orphan-rule wrapper for the SUT. Zero-cost — exists only so the
/// per-variant impls in this crate have a local type at the head of
/// `Sut` position. Wraps any `&mut S` where `S` satisfies whatever
/// capabilities the variants need.
///
/// Construct at the dispatch site: `LayoutSut::new(&mut my_session)`.
pub struct LayoutSut<'a, S: ?Sized> {
    inner: &'a mut S,
}

impl<'a, S: ?Sized> LayoutSut<'a, S> {
    pub fn new(inner: &'a mut S) -> Self {
        Self { inner }
    }
    /// Borrow the underlying session.
    pub fn inner(&mut self) -> &mut S {
        self.inner
    }
}

impl<S: Clickable + ?Sized> Clickable for LayoutSut<'_, S> {
    fn click_at_element(&mut self, element_id: &str) {
        self.inner.click_at_element(element_id);
    }
}

impl<S: LiveBlockSink + ?Sized> LiveBlockSink for LayoutSut<'_, S> {
    fn deliver_block_content_loaded(&mut self, block_id: &str) {
        self.inner.deliver_block_content_loaded(block_id);
    }
}

/// Orphan-rule wrapper for the reference state. Same role as
/// [`LayoutSut`] but in the `Ref` position. Construct at the
/// generator / precondition call site: `LayoutRef::new(&state)`.
pub struct LayoutRef<'a, R: ?Sized> {
    inner: &'a R,
    _marker: PhantomData<&'a R>,
}

impl<'a, R: ?Sized> LayoutRef<'a, R> {
    pub fn new(inner: &'a R) -> Self {
        Self {
            inner,
            _marker: PhantomData,
        }
    }
}

impl<R: LayoutRefState + ?Sized> LayoutRefState for LayoutRef<'_, R> {
    fn switchable_handles(&self) -> &[crate::blueprint::BlockHandle] {
        self.inner.switchable_handles()
    }
    fn drawer_handles(&self) -> &[crate::blueprint::DrawerHandle] {
        self.inner.drawer_handles()
    }
    fn deferred_block_ids(&self) -> Vec<String> {
        self.inner.deferred_block_ids()
    }
    fn collapsible_target_ids(&self) -> Vec<String> {
        self.inner.collapsible_target_ids()
    }
    fn current_view_mode(&self, block_id: &str) -> Option<&str> {
        self.inner.current_view_mode(block_id)
    }
    fn drawer_is_open(&self, block_id: &str) -> bool {
        self.inner.drawer_is_open(block_id)
    }
}
