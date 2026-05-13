//! Concrete reference state for the layout PBT.
//!
//! Replaces the ad-hoc `compute_final_*` fold functions in `scenario.rs`
//! and the `EmptyRef` placeholder in the GPUI session. One struct carries
//! every UI-state dimension a `UiInteraction` can mutate; each variant's
//! `TransitionImpl::apply_to_ref` updates the relevant field. After
//! replaying the full action vec, `SceneState` carries the final UI state
//! the reference mount should pre-apply.
//!
//! Adding a new dimension is one field here plus filling in
//! `apply_to_ref` on the new variant's `TransitionImpl` — no new
//! `compute_final_*` function, no new `StepInput::Mount` field.

use std::collections::{HashMap, HashSet};

use crate::blueprint::{BlockHandle, CollapsibleHandle, DrawerHandle};
use crate::sut::LayoutRefState;
use crate::ui_interaction::UiInteraction;

/// Reference state for the layout PBT. Tracks every UI-state dimension
/// each `UiInteraction` variant can mutate, plus the static handle
/// inventories the per-variant `weighted_generator`s need.
#[derive(Debug, Clone, Default)]
pub struct SceneState {
    /// Static handle inventories from the blueprint. Populated at
    /// construction; never mutated by `apply_to_ref`.
    pub switchable: Vec<BlockHandle>,
    pub drawers: Vec<DrawerHandle>,
    pub collapsibles: Vec<CollapsibleHandle>,

    /// Mutated by transitions. Empty entries mean "default state":
    ///   - missing mode → handle's initial mode (typically index 0)
    ///   - missing drawer entry → open
    ///   - missing collapsible entry → expanded
    ///   - missing delivered entry → not yet delivered
    pub active_modes: HashMap<String, String>,
    pub drawer_open: HashMap<String, bool>,
    pub expanded: HashMap<String, bool>,
    pub delivered_blocks: HashSet<String>,
}

impl SceneState {
    /// Build an empty SceneState carrying the handle inventories from
    /// `(switchable, drawers, collapsibles)` but no state mutations.
    /// Use this for the **test** mount (initial state — defaults apply).
    pub fn from_handles(
        switchable: Vec<BlockHandle>,
        drawers: Vec<DrawerHandle>,
        collapsibles: Vec<CollapsibleHandle>,
    ) -> Self {
        Self {
            switchable,
            drawers,
            collapsibles,
            ..Self::default()
        }
    }

    /// Build a SceneState with the final state after replaying every
    /// action. Use this for the **reference** mount.
    pub fn replay(
        switchable: Vec<BlockHandle>,
        drawers: Vec<DrawerHandle>,
        collapsibles: Vec<CollapsibleHandle>,
        actions: &[UiInteraction],
    ) -> Self {
        let mut state = Self::from_handles(switchable, drawers, collapsibles);
        for action in actions {
            state.apply(action);
        }
        state
    }

    /// Apply one `UiInteraction` to the state. The shared
    /// `TransitionImpl::apply_to_ref` slot exists for this, but
    /// `LayoutRef<'_, R>` wraps `&R` (read-only — generator-side use),
    /// so state-mutating reference replay routes through this match
    /// directly. Per-variant logic lives next to the click semantics
    /// in `crate::transitions::*::apply_to_ref` comments; this match
    /// is the canonical dispatcher.
    pub fn apply(&mut self, action: &UiInteraction) {
        match action {
            UiInteraction::SwitchViewMode(t) => {
                self.active_modes
                    .insert(t.block_id.clone(), t.target_mode.clone());
            }
            UiInteraction::ToggleDrawer(t) => {
                let cur = self.drawer_open.get(&t.block_id).copied().unwrap_or(true);
                self.drawer_open.insert(t.block_id.clone(), !cur);
            }
            UiInteraction::ToggleCollapse(t) => {
                let cur = self.expanded.get(&t.target_id).copied().unwrap_or(true);
                self.expanded.insert(t.target_id.clone(), !cur);
            }
            UiInteraction::DeliverBlockContent(t) => {
                self.delivered_blocks.insert(t.block_id.clone());
                // Convention from the old `compute_final_modes`: a
                // delivered block's mode becomes "loaded" unless a
                // later SwitchViewMode overrides it.
                self.active_modes
                    .entry(t.block_id.clone())
                    .or_insert_with(|| "loaded".to_string());
            }
        }
    }

    /// Drawers whose final state differs from default (open). Equivalent
    /// to the old `compute_final_drawer_states` output.
    pub fn closed_drawers(&self) -> HashMap<String, bool> {
        self.drawer_open
            .iter()
            .filter(|(_, &open)| !open)
            .map(|(id, _)| (id.clone(), false))
            .collect()
    }

    /// Collapsibles whose final state differs from default (expanded).
    /// Returned as `target_id → collapsed=true` so the Mount-side
    /// pre-apply only walks rows that need flipping.
    pub fn collapsed_rows(&self) -> HashMap<String, bool> {
        self.expanded
            .iter()
            .filter(|(_, &expanded)| !expanded)
            .map(|(id, _)| (id.clone(), true))
            .collect()
    }
}

impl LayoutRefState for SceneState {
    fn switchable_handles(&self) -> &[BlockHandle] {
        &self.switchable
    }
    fn drawer_handles(&self) -> &[DrawerHandle] {
        &self.drawers
    }
    fn deferred_block_ids(&self) -> Vec<String> {
        // Handles whose mode list includes "empty" (the deferred-content
        // placeholder convention) and that haven't been delivered yet.
        self.switchable
            .iter()
            .filter(|h| h.mode_names.iter().any(|m| m == "empty"))
            .map(|h| h.block_id.clone())
            .filter(|id| !self.delivered_blocks.contains(id))
            .collect()
    }
    fn collapsible_target_ids(&self) -> Vec<String> {
        self.collapsibles
            .iter()
            .map(|c| c.target_id.clone())
            .collect()
    }
    fn current_view_mode(&self, block_id: &str) -> Option<&str> {
        self.active_modes.get(block_id).map(String::as_str)
    }
    fn drawer_is_open(&self, block_id: &str) -> bool {
        self.drawer_open.get(block_id).copied().unwrap_or(true)
    }
}
