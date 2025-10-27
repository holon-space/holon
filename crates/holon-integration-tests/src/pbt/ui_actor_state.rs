//! Tier-3 UI actor fragment of the PBT reference model (ADR 0004 / 0006).
//!
//! `UIActorState` holds the viewer/session state owned by the UI actor. Per
//! ADR 0006 (known weakness #3) this state has two distinct lifetimes, split
//! into sub-fragments in Phase 6:
//!
//! - [`UITabState`] — **per-tab, ephemeral**: focus, cursor, the active-editor
//!   mirror, expand/drawer widget state, and per-region navigation history.
//!   Dies with the viewport; never synced.
//! - [`UIUserState`] — **per-user**: open pins and the active view profile.
//!   Persists for the user and is the natural seam for a future cross-device
//!   sync adapter (ADR 0006 #3) — keeping it separate avoids a re-split when
//!   multi-device sync arrives.
//!
//! Per ADR 0004 the whole fragment must **vanish when the UI isn't wired** —
//! isolating it here lets a non-UI wiring drop it instead of carrying dead
//! fields. Naming per ADR 0006 (`UIActorState`). Distinct from the domain
//! fragment ([`super::reference_domain_state::ReferenceDomainState`]) and the
//! production UI state — this is the PBT reference model's prediction of UI
//! actor state.

use std::collections::HashMap;
use std::collections::HashSet;

use holon_api::Region;
use holon_api::entity_uri::EntityUri;

use super::reference_state::ActiveEditor;
use super::reference_state::CursorPosition;
use super::reference_state::NavigationHistory;
use super::reference_state::OpenPinEntry;

/// Per-tab, ephemeral UI state — focus, cursor, widget open-state, and the
/// per-region navigation history. Never synced; recreated per viewport.
#[derive(Debug, Clone)]
pub struct UITabState {
    /// Navigation history per region (for back/forward navigation)
    pub navigation_history: HashMap<Region, NavigationHistory>,

    /// Mirrors SQLite's `navigation_history.id AUTOINCREMENT` counter.
    /// Bumped on every INSERT (not on UPDATE-only paths).
    pub next_history_id: i64,

    /// Currently focused entity ID per region (set by ClickBlock, updated by
    /// ArrowNavigate). Absent means no block is focused in that region.
    pub focused_entity_id: HashMap<Region, EntityUri>,

    /// Globally focused block mirror of `UiState.focused_block`. Updated by
    /// `NavigateFocus` to the navigation target.
    pub focused_block: Option<EntityUri>,

    /// Cursor position in the focused block per region.
    pub focused_cursor: HashMap<Region, CursorPosition>,

    /// Block URIs whose `expand_toggle` widget is currently expanded. Empty at
    /// startup. Mutated by `ExpandToggle` (insert) / `ToggleCollapse` (remove).
    pub expanded_toggles: HashSet<EntityUri>,

    /// Open/closed state per drawer (keyed by drawer block_id). Mirrors the
    /// production `widget_open` table. Mutated by `ToggleDrawer`.
    pub drawer_open: HashMap<String, bool>,

    /// Mirror of the GPUI editor's live `InputState` for the focused
    /// EditableText. `Some` after `FocusEditableText` and until focus moves
    /// elsewhere. Drives the commit-then-mutate contract for chord transitions.
    pub active_editor: Option<ActiveEditor>,

    /// Every block that has been a `NavigateFocus` target this app session.
    /// Budget model only: the FIRST navigation to a root renders it for the
    /// first time, which creates its watch matviews (ensure_view create path:
    /// 2 sqlite_master checks + DDL + initial SELECT per view); revisits hit
    /// the known-views cache. `inv-sql-budget` reads
    /// [`Self::last_navigate_first_visit`] to grant the creation allowance.
    pub seen_focus_targets: HashSet<EntityUri>,

    /// Whether the most recent `NavigateFocus` targeted a block not in
    /// [`Self::seen_focus_targets`] at apply time. Recorded by
    /// `NavigateFocus::apply_to_ref` because `expected_sql` only sees the
    /// post-apply state (where the target is already inserted).
    pub last_navigate_first_visit: bool,
}

impl UITabState {
    pub fn new() -> Self {
        Self {
            navigation_history: HashMap::new(),
            next_history_id: 1,
            focused_entity_id: HashMap::new(),
            focused_block: None,
            focused_cursor: HashMap::new(),
            expanded_toggles: HashSet::new(),
            drawer_open: HashMap::new(),
            active_editor: None,
            seen_focus_targets: HashSet::new(),
            last_navigate_first_visit: false,
        }
    }
}

impl Default for UITabState {
    fn default() -> Self {
        Self::new()
    }
}

/// Per-user UI state — open pins and the active view profile. Persists for the
/// user and is the seam for a future cross-device sync adapter (ADR 0006 #3).
#[derive(Debug, Clone)]
pub struct UIUserState {
    /// Open `navigation_history` rows per region (`closed_at IS NULL`).
    /// Mirrors the rows the `focus_roots` matview projects from.
    pub open_pins: HashMap<Region, Vec<OpenPinEntry>>,

    /// Monotonic logical timestamp for `OpenPinEntry::added_ts_logical`.
    pub next_pin_ts: u64,

    /// Current view filter ("all", "main", "sidebar").
    pub current_view: String,
}

impl UIUserState {
    pub fn new() -> Self {
        Self {
            open_pins: HashMap::new(),
            next_pin_ts: 1,
            current_view: "all".to_string(),
        }
    }
}

impl Default for UIUserState {
    fn default() -> Self {
        Self::new()
    }
}

/// UI actor (Tier-3) state extracted from `ReferenceState` (ADR 0004 Phase 3),
/// split per-tab / per-user in Phase 6 (ADR 0006 #3).
#[derive(Debug, Clone)]
pub struct UIActorState {
    /// Per-tab, ephemeral UI state (focus, cursor, widgets, nav history).
    pub tab: UITabState,

    /// Per-user UI state (pins, active view) — future sync-adapter seam.
    pub user: UIUserState,
}

impl UIActorState {
    pub fn new() -> Self {
        Self {
            tab: UITabState::new(),
            user: UIUserState::new(),
        }
    }
}

impl Default for UIActorState {
    fn default() -> Self {
        Self::new()
    }
}
