//! PBT capability traits — the reference-side (`Ref*`) and SUT-side (`Sut*`)
//! read/write surfaces the composed PBT catalog and `wide_e2e` runner bind on.
//!
//! @pbt kind cap-surface
//! @pbt covers all-slices — the component-oriented capability trait vocabulary
//!   every SUT arm and reference implements; selection is by presence of these.
//!
//! Each cap is a narrow trait hosted on `CapMap` (via `#[capmap_adapter]`, or
//! emitted in pairs by `capability_pair!`). Reference impls live in
//! `reference_capabilities.rs`; SUT impls in the composed components.
//! Pure-slice impls return constants; the wide PBT delegates to
//! `ReferenceState` / the real SUT.
//!
//! ## Three axes × two access modes (the editor/block/focus core)
//!
//! - **BlockTree**: in-memory block structure (parent/child, sort order,
//!   content, tags). `RefBlockTree` (read) / `RefBlockTreeMut` (write).
//! - **EditorMirror**: active-editor text + cursor mirror — what the GPUI
//!   `InputState` shows. `RefEditorMirror` / `RefEditorMirrorMut`.
//! - **Focus**: per-region focused block id + cursor position. `RefFocus` /
//!   `RefFocusMut`.
//!
//! Plus [`RefLifecycle`] for gate predicates (`app_started`,
//! `has_editor_buffer`, …) that transitions check, and the SUT write mirror
//! ([`SutBlockTreeWrite`], [`SutEditorMirrorWrite`], [`SutFocusWrite`],
//! [`SutQuiesce`]). SUT methods take only what they need — no `ref_state` leak
//! (the SUT keeps its `doc_uri_map` and similar state via interior mutability).
//!
//! Beyond that core, the file hosts the full projection/renderer/driver/Loro
//! cap set the wide catalog needs; each trait's own doc explains what it
//! observes and which invariants bind it.

use std::collections::BTreeSet;
use std::time::Duration;

pub use holon_api::EdgeFieldUpdate;
pub use holon_api::EntityUri;

/// Block identifier carried in capability-trait signatures. Aliased to
/// the real domain type [`holon_api::EntityUri`] — the wide PBT and the
/// pure slice both construct ids via `EntityUri::parse` / `EntityUri::block`,
/// so no boundary translation is needed. Kept as an alias (rather than a
/// bare `EntityUri`) so the capability surface reads as "block id" at the
/// call sites and so the type can be revisited centrally.
pub type CapBlockId = holon_api::EntityUri;

/// Symbolic region. Wide PBT uses `holon_api::Region` (Main / Sidebar);
/// pure slice has only a single region — its impl ignores the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CapRegion {
    Main,
    Sidebar,
    /// Used by impls that have no region distinction.
    Single,
}

/// Cursor position in the editor mirror. Wide PBT carries `line`+`column`
/// to mirror GPUI; pure slice tracks byte offset only. Concrete impls
/// adapt; the trait carries the structural shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CapCursor {
    pub line: usize,
    pub column: usize,
}

// ─── Reference-side: Advice ───────────────────────────────────────────

/// Expected advice rows for one anchor (read-time contract of ADR 0022's
/// advice matview: suppression anti-join + top-K happen at read time).
/// Total: `scored` is empty and `k == 0` when no active rule targets the
/// anchor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdviceExpectation {
    /// Eligible candidates AFTER the suppression anti-join, as
    /// (candidate id, shared-tag count), sorted by count DESC (tie order
    /// unspecified).
    pub scored: Vec<(String, u32)>,
    /// The rule's read-time top-K. 0 iff no active rule matches the anchor.
    pub k: u8,
}

/// Reference-side advice-weave contract for the `advice rows woven` keystone
/// invariant. `#[capmap_adapter]` hosts it on `CapMap` exactly like
/// [`RefBlockTree`] (sync, owned returns → no `#[async_trait]`); ids are
/// rendered as `EntityUri::as_str()` strings, the same form `block_raw.id`
/// carries in the SUT so the anchor/candidate strings compare directly against
/// the advice matview.
#[holon_macros::capmap_adapter]
pub trait RefAdvice {
    /// Total over all anchors: empty expectation when no rule matches.
    fn advice_expectation(&self, anchor: &str) -> AdviceExpectation;
    /// The matview-level contract: ALL (anchor, candidate, shared_tag_count)
    /// pairs of the single active rule, WITHOUT suppression and WITHOUT top-K
    /// (those are read-time). Empty when no active rule exists.
    fn advice_matview_rows(&self) -> Vec<(String, String, u32)>;
    /// Name of the matview the single active rule synthesizes
    /// (`advice_rule_{slug}`), or `None` when no active rule exists. The
    /// SQL-level twin `inv-advice-matview-matches-ref` compares this against
    /// the `advice_rule_%` matviews actually present in the SUT's
    /// `sqlite_master`.
    fn advice_matview_name(&self) -> Option<String>;
}

/// SUT-side observation of the synthesized advice matviews (ADR 0022 step-6).
/// One materialized view per active rule, named `advice_rule_{slug}`,
/// projecting `(anchor_id, lesson_id, shared_tag_count)` — the pre-suppression,
/// un-capped matview contract. The `inv-advice-matview-matches-ref` twin reads
/// this and compares it against [`RefAdvice::advice_matview_name`] +
/// [`RefAdvice::advice_matview_rows`]. Until synthesis lands (step 6) the SUT
/// has no such matview, so this observes an empty set — that IS the
/// observed-absent state the twin flips out of once synthesis wires the DDL.
#[holon_macros::capmap_adapter]
pub trait SutAdviceMatview {
    /// Every materialized view named `advice_rule_%` present in the SUT's
    /// `sqlite_master`, paired with its full row set as
    /// `(anchor_id, lesson_id, shared_tag_count)`. Empty when synthesis has
    /// created none. Read AFTER CDC quiescence.
    async fn advice_matviews(&self) -> Vec<(String, Vec<(String, String, u32)>)>;
}

/// SUT-side differential observation for
/// `inv-matview-consistent-with-recompute`. For every `CREATE MATERIALIZED
/// VIEW` in the SUT's `sqlite_master`, reads the IVM-maintained matview
/// contents AND re-executes the view's own defining SELECT now, returning both
/// as canonically-sorted multisets so the body is a pure comparison. A
/// persistent per-view divergence is an IVM-vs-recompute bug (the "22-vs-20
/// matview DUP" class); transient CDC lag self-heals inside the
/// body's bounded-wait window. Placeholder / context-param views are skipped
/// inside the impl (disclosed, not faked).
#[holon_macros::capmap_adapter]
pub trait SutMatviews {
    /// Per MATERIALIZED view: (name, matview_rows, recompute_rows).
    /// Both row sets canonically sorted; multiset semantics (dups kept).
    async fn matview_recompute_snapshot(&self)
    -> Vec<(String, Vec<Vec<String>>, Vec<Vec<String>>)>;
}

// ─── Reference-side: BlockTree ────────────────────────────────────────

/// Read-side block-tree queries used by Phase 5 T0 transitions and their
/// generators.
///
/// `#[capmap_adapter]` hosts this on `CapMap`. The trait is fully sync, so no
/// `#[async_trait]` wrapper is emitted (existing `impl RefBlockTree for
/// ReferenceState` is untouched); the borrow-returning `block_content ->
/// Option<&str>` forwards through `CapMap::expect_ref` so it doesn't dangle.
#[holon_macros::capmap_adapter] // sync trait → no async-trait; emits CapName + `impl … for CapMap`
pub trait RefBlockTree {
    /// Returns block content text. `None` if the block does not exist.
    fn block_content(&self, id: &EntityUri) -> Option<&str>;

    /// True if the block exists and is a Text-typed block (the only kind
    /// editor transitions care about).
    fn is_text_block(&self, id: &EntityUri) -> bool;

    /// Editable Text descendants of the focus root in `Main` region.
    /// Empty in pure slice if the test fixture didn't seed any.
    fn main_editable_descendants(&self) -> Vec<EntityUri>;

    /// Block ids of the current focus roots in `region`. Wide PBT
    /// computes from `expected_focus_root_ids`; pure slice may just
    /// return the root id of its single doc.
    fn focus_root_ids(&self, region: CapRegion) -> BTreeSet<EntityUri>;

    /// Sibling navigation.
    fn previous_sibling(&self, id: &EntityUri) -> Option<EntityUri>;
    fn next_sibling(&self, id: &EntityUri) -> Option<EntityUri>;

    /// Parent of `id`. `None` if `id` is root or has a sentinel parent
    /// (wide PBT: `EntityUri::is_no_parent` / `is_sentinel`; pure slice:
    /// `parent: None`).
    fn parent_of(&self, id: &EntityUri) -> Option<EntityUri>;

    /// Grandparent for outdent.
    fn grandparent(&self, id: &EntityUri) -> Option<EntityUri>;

    /// Children of a parent, sorted by sort_key. Returns ids only —
    /// callers join back through `block_content` if they need data.
    fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri>;

    /// True if `id` is a descendant of any ancestor in `ancestors`.
    ///
    /// A BARE parent-chain walk — it does NOT decide main-panel visibility. Use
    /// [`Self::main_panel_renders`] for that: the panel query stops descending
    /// at non-root pages and truncates at a depth cap, so this walk reports
    /// blocks the panel does not show.
    fn is_descendant_of_any(&self, id: &EntityUri, ancestors: &BTreeSet<EntityUri>) -> bool;

    /// Whether the main panel actually renders a row for `id`, under the SAME
    /// traversal the compiled main-panel query applies: descend from the Main
    /// focus root(s), stop at any NON-ROOT page, truncate at the panel's depth
    /// cap. The one predicate for "a user could click this row" and for "an
    /// invariant may demand this row"; a bare ancestor check over-approximates
    /// both and produces click targets that never bind and required rows prod
    /// is right to omit.
    fn main_panel_renders(&self, id: &EntityUri) -> bool;

    /// Layout blocks (the layout scaffolding the user can't focus into).
    /// Wide PBT: `layout_blocks` set; pure slice: empty.
    fn is_layout_block(&self, id: &EntityUri) -> bool;

    /// True if `id` exists and is focusable (i.e. not a layout block,
    /// not immutable, has the right content type).
    fn is_focusable(&self, id: &EntityUri) -> bool;

    /// True if `id` is in the "no content update" set — render sources,
    /// query sources, profile blocks. Wide PBT consults
    /// `layout_blocks.render_source_ids` + `layout_blocks.query_source_ids`
    /// + `profile_block_ids`. Pure slice has no such concept → returns `false`.
    fn is_no_content_update(&self, id: &EntityUri) -> bool;

    /// True if `id` is a Page block (tagged `Page`). Mirrors
    /// `Block::is_page()`. Pure slice has no pages → returns `false`.
    fn is_page_block(&self, id: &EntityUri) -> bool;

    /// True if `id` exists and is a Source-typed block (rhai/query source).
    /// `ApplyMutation`'s Move/Create preconditions gate targets on
    /// `content_type != Source`. Pure slice has no source blocks → default
    /// `false` (so a defaulted impl on `EditorPureRef`/toy refs stays correct).
    fn is_source_block(&self, _: &EntityUri) -> bool {
        false
    }

    /// All block ids tracked by the reference model, EXCLUDING seed
    /// blocks (those with sentinel/no_parent docs — they're inserted
    /// via direct SQL, never reverse-synced to Loro, and don't appear
    /// in the matview the wide PBT compares against).
    ///
    /// Used by `inv-block-ids-match-ref` to compare against
    /// `SutSqlProjection::all_block_ids()` for set-equality drift
    /// detection at the storage layer.
    fn all_non_seed_block_ids(&self) -> BTreeSet<EntityUri>;
}

/// Block-tree mutations. Concrete impls maintain whatever bookkeeping
/// they need (sort_key generation, undo snapshots, focus follow-ups);
/// the trait only commits to the shape of the operation.
pub trait RefBlockTreeMut: RefBlockTree {
    /// Push the current state onto the undo stack. Wide PBT: real
    /// snapshot; pure slice: may be a no-op if undo isn't tested.
    fn push_undo_snapshot(&mut self);

    /// Set the content text of `id`. Used by `commit_active_editor_if_changed`
    /// and any future direct-write transitions.
    fn set_block_content(&mut self, id: &EntityUri, text: &str);

    /// Split `id` at `position`. Returns the id of the LOWER block — the one
    /// holding the tail text, which is also the split's focus target. Identity
    /// follows the text: that is the newly-minted block for `position > 0`, but
    /// `id` itself at `position == 0` (there the minted id goes to the EMPTY
    /// block inserted ABOVE, so references to `id` keep resolving to the text).
    fn split_block(&mut self, id: &EntityUri, position: usize) -> EntityUri;

    /// Join `id` into its previous sibling (or parent if no previous
    /// sibling). Returns the cursor position of the join point in the
    /// merged block's content.
    fn join_block(&mut self, id: &EntityUri) -> usize;

    /// Indent `id` — re-parent under previous sibling.
    fn indent(&mut self, id: &EntityUri);

    /// Outdent `id` — move up to grandparent level.
    fn outdent(&mut self, id: &EntityUri);

    /// Re-parent `id` under `new_parent`, placing it after `after` (or
    /// first if `after` is None). Used by Indent/Outdent helpers when
    /// they don't want to bake the parent-discovery logic into the
    /// transition body. The wide-PBT impl is
    /// `ReferenceState::move_block`.
    fn move_block(&mut self, id: &EntityUri, new_parent: EntityUri, after: Option<&EntityUri>);

    /// Swap two siblings (used by MoveUp / MoveDown).
    fn swap_siblings(&mut self, a: &EntityUri, b: &EntityUri);

    /// Undo the last mutation (pop undo→redo) and reset every region cursor to
    /// start — the whole `UndoLastMutation` reference effect. Defaults to a
    /// no-op for slices that don't model an undo stack.
    fn undo_last_and_reset_cursors(&mut self) {}

    /// Redo the last undone mutation (pop redo→undo) and reset every region
    /// cursor to start — the whole `Redo` reference effect. Defaults to a
    /// no-op for slices that don't model a redo stack.
    fn redo_last_and_reset_cursors(&mut self) {}

    /// Re-mint a block: swap its identity for a FRESH synthetic id (keeping
    /// content, parent, and position) and re-parent its children onto the new
    /// id. Returns the new synthetic id. Models the reference side of the R2
    /// id-less-reconcile CHURN: when `StaleExternalRewrite` replays a doc with
    /// non-unique / empty content, `tiered_match` cannot uniquely remap those
    /// blocks, so the SUT re-mints them; the oracle must predict the same so
    /// the per-tick synthetic→real reconcile pairs 1:1 instead of
    /// panicking. Only `StaleExternalRewrite` (composed environment)
    /// reaches this.
    fn remint_block(&mut self, old_id: &EntityUri) -> EntityUri;

    /// The block's task-state keyword, if this reference models task state at
    /// all. Only the live task-keyword promotion reads it.
    fn block_task_state(&self, _: &EntityUri) -> Option<String> {
        None
    }

    /// The task-keyword vocabulary that governs `id`: its owning document's
    /// `#+TODO:` declaration, else the defaults.
    ///
    /// DERIVED from the drawn document, never a copy of the SUT's constant —
    /// a model that hardcodes the same vocabulary prod hardcodes agrees with
    /// prod's wrong answer and cannot convict it.
    fn block_task_vocabulary(&self, _: &EntityUri) -> holon_org_format::TaskKeywordVocabulary {
        holon_org_format::TaskKeywordVocabulary::default()
    }

    /// What an editor OPENED on `id` shows: the block's vault syntax
    /// (`TODO milk`) for the task-keyword facet, its stored content otherwise.
    ///
    /// The editable surface is a projection, not the content column, so every
    /// modelled editor seed reads it here. A reference that does not model task
    /// state answers with the content, which is exactly right for it.
    fn editor_surface_text(&self, id: &EntityUri) -> String {
        self.block_content(id).unwrap_or_default().to_string()
    }

    /// Commit an editor buffer through the SOURCE channel: parse `source` under
    /// the block's own vocabulary and land `content` AND `task_state` from it,
    /// clearing the task state when the source carries no keyword.
    ///
    /// This is the STORE's rule, not the editor's — the model has no separate
    /// promotion step, which is the whole point of the source projection.
    /// Defaults to a plain content write for a reference that does not model
    /// task state.
    fn commit_editor_source(&mut self, id: &EntityUri, source: &str) {
        self.set_block_content(id, source);
    }

    /// Re-seed an idle editor open on `id` from the authority as the surface
    /// shows it, keeping the one divergence prod's echo discriminator
    /// recognises as its own write (trailing whitespace). No-op for a reference
    /// that models no editor.
    fn refresh_clean_editor_surface(&mut self, id: &EntityUri) {
        let _ = id;
    }
}

// ─── Reference-side: EditorMirror ────────────────────────────────────

/// Read-side active-editor state.
///
/// `#[capmap_adapter]` hosts this on `CapMap` (sync trait → no
/// `#[async_trait]`, `impl RefEditorMirror for ReferenceState` untouched). The
/// borrow-returning `active_editor_text -> Option<&str>` forwards through
/// `CapMap::expect_ref`.
#[holon_macros::capmap_adapter]
pub trait RefEditorMirror {
    /// Block id whose editor is currently active, or `None` if no editor
    /// is open. Pure slice typically has this populated by a setup
    /// transition; wide PBT mirrors GPUI's `InputState`.
    fn active_editor_block(&self) -> Option<EntityUri>;

    /// Live in-memory editor text. Pre-blur, this can diverge from
    /// `block_content(active_editor_block())` — the divergence is what
    /// surfaces split-with-pending-edit bugs.
    fn active_editor_text(&self) -> Option<&str>;

    /// Cursor byte offset within `active_editor_text`.
    fn active_editor_cursor(&self) -> Option<usize>;

    /// True iff modeled typing/deleting touched the active editor's text
    /// since it opened (or since the last commit). Distinguishes
    /// user-authored pending text (commits on blur / at structural commit
    /// points) from a mirror that merely went stale against an external
    /// change (prod's data subscription refreshes idle editors; committing
    /// a stale mirror writes old text into the ref). Default `false` keeps
    /// lean slice models, which never type, on the never-commits path.
    fn active_editor_dirty(&self) -> bool {
        false
    }
}

/// Editor-mirror mutations. Apply to whichever editor is active.
pub trait RefEditorMirrorMut: RefEditorMirror {
    fn type_chars(&mut self, text: &str);
    fn delete_backward(&mut self, count: usize);
    fn move_cursor(&mut self, byte_position: usize);

    /// Replace the active editor's text and caret wholesale, because the view
    /// model rewrote its own buffer instead of adopting what was typed (a live
    /// task-keyword promotion strips the keyword out of the visible text).
    /// Required, not defaulted: a silent no-op here would leave the mirror
    /// holding text prod no longer shows.
    fn reseed_active_editor(&mut self, text: &str, cursor: usize);

    /// Clear the dirty flag after a commit. Default no-op for models
    /// without dirty tracking.
    fn mark_active_editor_committed(&mut self) {}

    /// Record how many block MERGES the backspace run just performed — a
    /// backspace at caret 0 joins into the predecessor instead of deleting a
    /// character, and `inv-sql-budget` charges each merge the `JoinBlock`
    /// budget. Recorded at apply time because the budget only ever sees the
    /// POST-apply reference, where a merge and a mid-line delete are
    /// indistinguishable. Default no-op for models that carry no SQL budget.
    fn note_backspace_joins(&mut self, joins: usize) {
        let _ = joins;
    }
}

// ─── Reference-side: Focus ───────────────────────────────────────────

/// `inv-navigation-focus`'s comparator, extracted verbatim from the former
/// hand-written invariant body so `capability_pair!`'s `#[compare(with = ..)]`
/// can auto-derive the wiring. The `current_focus` matview's per-region focus
/// (SUT) must match the reference's navigation focus.
///
/// A pure two-value function of the SUT rows and reference rows (no auxiliary
/// cap reads) — a faithful 1:1 move of the old
/// `bodies/navigation_focus::InvNavigationFocus::check` compare step. Both are
/// `(region, block_id)` where `block_id` is `None` for a region navigated home;
/// the ref rows are pre-resolved into SUT id space by `with_resolved_doc_uris`.
pub fn compare_navigation_focus(
    sut_rows: &[(String, Option<String>)],
    ref_rows: &[(String, Option<String>)],
) -> Result<(), String> {
    use std::collections::HashMap;
    use std::collections::HashSet;

    // region -> block_id (None = NULL/home). Presence of the key = the matview
    // has a row for that region.
    let sut_map: HashMap<String, Option<String>> = sut_rows.iter().cloned().collect();

    for (region, expected) in ref_rows {
        let has_row = sut_map.contains_key(region);
        let row_block_id = sut_map.get(region).cloned().flatten();
        match (has_row, expected) {
            (true, Some(exp)) => {
                if row_block_id.as_deref() != Some(exp.as_str()) {
                    return Err(format!(
                        "[inv-navigation-focus] region '{region}': expected focus {exp:?}, \
                         matview has {row_block_id:?}"
                    ));
                }
            }
            (true, None) => {
                if row_block_id.is_some() {
                    return Err(format!(
                        "[inv-navigation-focus] region '{region}': expected home (no focus), \
                         matview has {row_block_id:?}"
                    ));
                }
            }
            (false, None) => {}
            (false, Some(exp)) => {
                return Err(format!(
                    "[inv-navigation-focus] region '{region}' should have focus on {exp:?} but \
                     has no row in the current_focus matview"
                ));
            }
        }
    }

    // Reverse direction: matview regions must be ⊆ ref regions — a focused row
    // for a region the reference never navigated is a ghost. NULL rows (focus
    // cleared / home) for an untracked region carry no focus and are tolerated.
    let ref_regions: HashSet<&String> = ref_rows.iter().map(|(region, _)| region).collect();
    for (region, block_id) in &sut_map {
        if let Some(ghost) = block_id {
            if !ref_regions.contains(region) {
                return Err(format!(
                    "[inv-navigation-focus] ghost row: current_focus matview has region \
                     '{region}' focused on {ghost:?}, but the reference has no navigation history \
                     for that region"
                ));
            }
        }
    }
    Ok(())
}

// `capability_pair!` single-sources the focus read duality: the SUT-side
// `current_focus`/`focus_roots` matview projection ([`SutFocus`],
// async, CDC-quiesced) and the reference navigation-focus model ([`RefFocus`],
// sync, owned). The `#[compare]` method auto-derives `inv-navigation-focus`
// (id preserved — it is a `WIDE_REQUIRED` invariant asserted by id in the
// slice teeth) via [`compare_navigation_focus`]; `focus_roots` fan-out stays a
// hand-written invariant (it reads three SUT sources + a ref source, so it is
// not a two-value compare) but its cap methods live here as `#[sut_only]` /
// `#[ref_only]`. The stem `Focus` yields the trait names `SutFocus` /
// `RefFocus`.
holon_macros::capability_pair! {
    /// Focus read surface: SUT `current_focus`/`focus_roots`/`navigation_history`
    /// projection (Turso, post-CDC-quiescence) vs the reference navigation-focus
    /// model. `#[capmap_adapter]`-equivalent glue hosts both on `CapMap` (the SUT
    /// trait async → `#[async_trait(?Send)]`; the reference trait fully sync/owned
    /// → plain trait). SUT registered only where navigation is actually driven
    /// through a `current_focus`/`focus_roots` projection (the frontend slice /
    /// `full_headless`); a storage-only slice does NOT register it, so the focus
    /// invariants honestly DESELECT there instead of passing vacuously.
    pub trait Focus {
        /// Rows of the `current_focus` matview as `(region, block_id)` (SUT) /
        /// per-region navigation focus (reference). `block_id` is `None` for a
        /// region navigated home (NULL in SQL). The reference keys by the SQL
        /// region strings — LeftSidebar/RightSidebar granularity that
        /// [`CapRegion`] collapses. Auto-compared by `inv-navigation-focus` via
        /// [`compare_navigation_focus`].
        #[compare(
            sut = current_focus_rows,
            ref = navigation_focus_rows,
            id = "inv-navigation-focus",
            layer = ViewModel,
            with = crate::capabilities::compare_navigation_focus
        )]
        fn current_focus_rows(&self) -> Vec<(String, Option<String>)>;

        /// Rows of the `focus_roots` matview as `(region, root_id)` — the
        /// convergent truth-check for `inv-focus-roots`' CDC-lag downgrade.
        #[sut_only]
        fn focus_roots_rows(&self) -> Vec<(String, String)>;

        /// Open rows of the BASE `navigation_history` table as `(region, block_id)`
        /// — exactly the set the `focus_roots` matview projects from
        /// (`WHERE closed_at IS NULL AND block_id IS NOT NULL`). Lets
        /// `inv-focus-roots` distinguish a genuine matview/IVM drift (base no
        /// longer has the row, matview still does) from a holon close-path bug
        /// (base still has the row open, so the matview is *correctly* showing it).
        #[sut_only]
        fn nav_history_open_rows(&self) -> Vec<(String, String)>;

        /// Expected focus-root ids per region as `(region_string, [root_id])`, for
        /// `inv-focus-roots`. Region strings match the `focus_roots` matview;
        /// already resolved into SUT id space by `with_resolved_doc_uris` (the
        /// `open_pins` block_ids it derives from are remapped there).
        #[ref_only]
        fn expected_focus_root_rows(&self) -> Vec<(String, Vec<String>)>;

        /// Currently focused block in `region`. Wide PBT: per-region map;
        /// pure slice: returns from a single field.
        #[ref_only]
        fn current_focus(&self, region: CapRegion) -> Option<EntityUri>;

        /// Cursor position of the focused block's editor (if known).
        #[ref_only]
        fn focused_cursor(&self, region: CapRegion) -> Option<CapCursor>;
    }
}

/// Focus mutations.
pub trait RefFocusMut: RefFocus {
    /// Set focus to `id` in `region`, resetting cursor to `cursor`.
    fn set_focus(&mut self, region: CapRegion, id: EntityUri, cursor: CapCursor);

    /// Clear focus if it currently points at a now-deleted block.
    fn clear_focus_if_deleted(&mut self, id: &EntityUri);

    /// Open an active editor on `id` with `content` and the caret at
    /// `cursor_byte`, replacing any prior active editor. Mirrors prod's split
    /// focus (ADR 0010): `split_block` returns the freshly-created block as the
    /// focus target at position 0 (op response, applied in-process), so a
    /// *subsequent* Enter splits the NEW block — not the block the prior
    /// `FocusEditableText` targeted.
    /// Without this the ref leaves `active_editor` stale and `PressKey(Enter)`
    /// splits the wrong block, diverging from prod (and the headless SUT once
    /// its `focused_block` settles). Default no-op for pure-slice reference
    /// machines that have no editor state.
    fn open_active_editor(&mut self, _: EntityUri, _: String, _: usize) {}

    /// Close the active editor (e.g. after a Backspace-at-0 join deletes the
    /// edited block — prod closes that block's editor). Counterpart of
    /// [`Self::open_active_editor`]; default no-op for editor-less refs.
    fn close_active_editor(&mut self) {}
}

// ─── Reference-side: calendar clock (ADR 0024 §6 AdvanceDay) ─────────

/// Reference-side view of the ambient calendar day and the journal-rule
/// output it predicts. The `AdvanceDay` transition advances the injected fake
/// clock; the model advances `today` in lockstep and predicts exactly one
/// journal block per distinct day the clock has visited (the boot day plus
/// every advanced-to day), idempotent under same-day re-ticks.
/// `#[capmap_adapter]` hosts the read side on the ref `CapMap` so a
/// journal-count invariant can read it; the mutation is a plain `RefClockMut`
/// (mutations are not capmap-hosted — `apply_to_ref` drives the concrete
/// `ReferenceState`).
#[holon_macros::capmap_adapter]
pub trait RefClock {
    /// The model's current local calendar day, `YYYY-MM-DD`.
    fn today(&self) -> String;

    /// How many distinct journal day-blocks the rule should have produced so
    /// far: the boot day plus every distinct day the clock has advanced to.
    fn expected_journal_day_count(&self) -> usize;

    /// Every distinct calendar day (`YYYY-MM-DD`) the clock has visited — the
    /// boot day plus each advanced-to day. Its cardinality equals
    /// [`Self::expected_journal_day_count`]; the set itself lets
    /// `inv-journal-one-per-day` assert the SUT's journal date-pages match the
    /// visited days exactly (title equality, not just count).
    fn visited_days(&self) -> std::collections::BTreeSet<String>;
}

/// Mutating half of [`RefClock`] — advancing the model's calendar day. Not
/// capmap-hosted (like `RefBlockTreeMut`): `apply_to_ref` mutates the concrete
/// `ReferenceState`.
pub trait RefClockMut: RefClock {
    /// Advance the model day by `days` (`0` = same-day re-tick, which the
    /// deterministic-id rule makes a no-op) and record the new distinct day.
    fn advance_day(&mut self, days: i64);
}

// ─── Reference-side: Sharing audience (ADR 0028 C2/H3) ───────────────

/// A sharing **audience**: the set of NON-OWNER members a container or a
/// block's policy extends visibility to. The owner is implicit and always a
/// member (never stored). `Audience::default()` = local-only (owner-only).
///
/// Members are opaque labels (device / recipient ids). The C2/H3 directional
/// alignment oracle compares these member sets: a block's *effective* audience
/// (the audience of the container it is observably in) must never
/// OVER-approximate its *policy* audience (owner intent) — i.e. effective ⊆
/// policy at every observable point (ADR 0028 §H3; the directional restatement
/// of the unsatisfiable-as-literal C2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Audience(pub BTreeSet<String>);

impl Audience {
    /// The owner-only audience (no extra members).
    pub fn local_only() -> Self {
        Self(BTreeSet::new())
    }

    /// Build from an iterator of member labels.
    pub fn from_members<I, S>(members: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self(members.into_iter().map(Into::into).collect())
    }

    /// The non-owner members.
    pub fn members(&self) -> &BTreeSet<String> {
        &self.0
    }

    /// True when only the owner is in the audience.
    pub fn is_local_only(&self) -> bool {
        self.0.is_empty()
    }

    /// Members present in `self` but ABSENT from `policy` — the
    /// over-approximation (leak) set. Empty ⇔ `self ⊆ policy` (aligned).
    pub fn over_approximation_of(&self, policy: &Audience) -> BTreeSet<String> {
        self.0.difference(&policy.0).cloned().collect()
    }
}

/// C2/H3 directional-alignment oracle surface (ADR 0028). Exposes, per block,
/// the POLICY audience (owner intent) and the EFFECTIVE audience (the audience
/// of the container the block is observably in). The keystone oracle
/// (`inv-audience-never-over-approximates`) asserts effective never
/// OVER-approximates policy — the correctness spine every future share
/// migration must keep green (widening = create-in-shared FIRST; narrowing =
/// delete-from-shared FIRST; quiescent form at an epoch barrier: membership =
/// policy extension).
///
/// Ref-only (no SUT twin yet): there is no share machinery in prod, so the
/// EFFECTIVE audience is derived from the reference model's
/// container-membership map, not from a SUT observation. When real crossings
/// land (Inc 4) the SUT side can observe effective audience via
/// `list_loro_documents` / `inspect_loro_blocks` (which peers replicate a doc)
/// and this becomes a SUT⇄ref differential; today it is the migration model's
/// own tripwire (same shape as `inv-no-page-under-non-page`).
///
/// `#[capmap_adapter]` hosts this on `CapMap` (sync trait → no async-trait) so
/// the bridged invariant can read it from the ref `CapMap`.
#[holon_macros::capmap_adapter]
pub trait RefAudience {
    /// The current sharing epoch. Crossings are ordered; at an epoch barrier
    /// the quiescent invariant is membership = policy extension.
    fn audience_epoch(&self) -> u64;

    /// Blocks with a non-local effective OR policy audience — the only blocks
    /// the over-approximation check is meaningful for (a local-only block is
    /// vacuously aligned: ∅ ⊆ ∅). Empty on a ref modeling no crossings (the
    /// default keystone), so the oracle is vacuously green there.
    fn shared_block_ids(&self) -> BTreeSet<EntityUri>;

    /// The owner-intended audience for this block. Absent ⇒ local-only.
    fn block_policy_audience(&self, block: &EntityUri) -> Audience;

    /// The effective audience = the audience of the container (doc) the block
    /// is observably in. Absent container / local-only doc ⇒ local-only.
    fn block_effective_audience(&self, block: &EntityUri) -> Audience;
}

// ─── Reference-side: Lifecycle (admin gates) ─────────────────────────

/// Setup/lifecycle predicates that wide-PBT transitions gate on.
/// Pure-slice impls return constants (always started, always set up,
/// loro off for pure-logic-only).
pub trait RefLifecycle {
    fn app_started(&self) -> bool;
    fn is_properly_setup(&self) -> bool;
    fn enable_loro(&self) -> bool;

    /// Whether this reference owns an **editor buffer** carrying uncommitted
    /// text — the headless atomic-editor capability. This is the single gate
    /// for the editor transitions (TypeChars / DeleteBackward / MoveCursor /
    /// FocusEditableText / PressKey), replacing the old pairing of
    /// `atomic_editor_enabled()` (env-var gated) and `enable_loro() ||
    /// real_editor_enabled()` (storage-coupled). Editor buffering is a property
    /// of the wired editor component, independent of *Loro-as-storage* (the
    /// CRDT can buffer text regardless of where blocks persist) and of any
    /// process env var. Defaults to `false`: a ref with no editor buffer
    /// never generates editor transitions.
    fn has_editor_buffer(&self) -> bool {
        false
    }

    /// The bounds-registry id of the ACTIVE sticky-accordion footer currently
    /// on screen, if any — the target the `WheelScroll` sticky-footer source
    /// scrolls over. `None` by default: a reference that models no sticky
    /// footer never generates the footer wheel source (the source activates
    /// when a Journals-shaped section stack is modelled — Inc E boundary).
    fn sticky_footer_element_id(&self) -> Option<String> {
        None
    }

    /// Whether a block-interaction transition (indent / drag / chord / …) can
    /// dispatch against `block_id` under the active main-panel layout: the
    /// block must be in the layout query's rendered set AND rendered with
    /// an interactive widget.
    ///
    /// The default layout queries `focus_root` (navigation-aware, transitive)
    /// and renders each block via `render_entity()` (operations + `draggable` +
    /// `editable_text`), so any focused-subtree block qualifies. A user
    /// `index.org` layout renders a possibly-different set (a `from children`
    /// query surfaces only the layout block's direct children; an all-blocks
    /// query surfaces everything) through a possibly-static template
    /// (`row(text(...))`, no operations) — the reference evaluates BOTH axes
    /// faithfully (see `ReferenceState::renders_block_interactively`) rather
    /// than blanket-excluding every custom layout. Defaults to `true`;
    /// `ReferenceState` overrides it.
    fn renders_block_interactively(&self, block_id: &EntityUri) -> bool {
        let _ = block_id;
        true
    }

    /// The previous-transition kind, for Markov weighting. Returns
    /// `None` on the first step or when the impl doesn't track history.
    fn last_transition_kind(&self) -> Option<&'static str>;

    /// The next synthetic-document id counter (`action.next_doc_id`). Read by
    /// `CreateDocument`'s generator to mint the `doc_<n>.org` filename.
    /// Defaults to `0`: a slice that never mints synthetic documents (the
    /// pure editor slice) has no counter to advance.
    fn next_doc_id(&self) -> usize {
        0
    }

    /// The next synthetic-block id counter (`domain.block_state.next_id`). Read
    /// by `BulkExternalAdd` (and `ApplyMutation`) to mint `bulk-<n>-<i>`
    /// block ids. Defaults to `0` for slices with no block-minting counter.
    fn next_block_id(&self) -> usize {
        0
    }

    /// Whether codepoint-level peer text edits are enabled (`PBT_MUTABLE_TEXT`
    /// process env gate). Read by `PeerCharEdit`'s precondition. This is a
    /// process-global gate, not per-ref state; the default reads the env var so
    /// every reference agrees without pinning a concrete state type.
    fn mutable_text_enabled(&self) -> bool {
        std::env::var("PBT_MUTABLE_TEXT")
            .ok()
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false)
    }

    /// Whether the undo stack is non-empty (`UndoLastMutation` precondition).
    /// Defaults to `false`: a slice with no undo stack never generates undo.
    fn has_undo_history(&self) -> bool {
        false
    }

    /// Whether the redo stack is non-empty (`Redo` precondition). Defaults to
    /// `false` for slices that don't model a redo stack.
    fn has_redo_history(&self) -> bool {
        false
    }
}

// ─── SUT-side traits (mirror of reference-side write traits) ─────────

/// SUT mutations on the block tree. Methods do NOT take `ref_state` —
/// concrete impls (e.g. wide-PBT `E2ESut`) keep any needed ref→SUT id
/// mapping in interior state (e.g. `doc_uri_map`).
///
/// `&self` + interior mutability (not `&mut self`): the underlying stores are
/// already interior-mutable (`MemoryBackend` = `Arc<RwLock>`, `E2ESut`'s
/// `Arc<Mutex>` fields), so `#[capmap_adapter]` hosts the write cap on `CapMap`
/// exactly like the read caps — the composed slice's `CapMap` *is* the
/// `SutTransitionTarget`.
#[holon_macros::capmap_adapter]
pub trait SutBlockTreeWrite {
    async fn apply_split_block(&self, id: &EntityUri, position: usize);
    async fn apply_join_block(&self, id: &EntityUri);
    async fn apply_indent(&self, id: &EntityUri);
    async fn apply_outdent(&self, id: &EntityUri);
    async fn apply_move_up(&self, id: &EntityUri);
    async fn apply_move_down(&self, id: &EntityUri);
}

/// SUT capability: write a block **edge field** (`tags`/`requires`, the
/// junction-backed set-valued attributes) on an EXISTING block, parameterized
/// over *which* field via [`EdgeFieldUpdate`] so neither is special-cased.
///
/// `&self` + interior mutability like the other write caps, so
/// `#[capmap_adapter]` hosts it on `CapMap` — the composed `CapMap` IS the
/// `SutTransitionTarget`. The realization calls the production edge-field
/// writers (`set_block_tags` / `set_block_requires` on the Loro backend — the
/// same functions the org re-scan reconciliation uses), so the write flows Loro
/// → `project()` → SQL exactly as production does. That is what lets the
/// composed `/matview` invariant observe a dropped edge-field re-projection
/// (e.g. H12: `blocks_differ` omitting `requires`).
#[holon_macros::capmap_adapter]
pub trait SutEdgeFieldWrite {
    async fn apply_set_edge_field(&self, id: &EntityUri, update: &EdgeFieldUpdate);
}

#[holon_macros::capmap_adapter]
pub trait SutEditorMirrorWrite {
    async fn apply_type_chars(&self, text: &str);
    async fn apply_delete_backward(&self, count: usize);
    async fn apply_move_cursor(&self, byte_position: usize);
}

/// Drive an `Arc`-shared write component through
/// `SutTransitionTarget::apply_to_sut` (which takes `&mut S`). The editor write
/// component is held behind `Arc` because the SAME instance is also registered
/// as the read-side `SutEditorMirrorRead` cap; the write methods are `&self`
/// (interior mutability), so forwarding through the shared `Arc` is sound.
#[async_trait::async_trait(?Send)]
impl<T: SutEditorMirrorWrite + ?Sized> SutEditorMirrorWrite for std::sync::Arc<T> {
    async fn apply_type_chars(&self, text: &str) {
        (**self).apply_type_chars(text).await
    }
    async fn apply_delete_backward(&self, count: usize) {
        (**self).apply_delete_backward(count).await
    }
    async fn apply_move_cursor(&self, byte_position: usize) {
        (**self).apply_move_cursor(byte_position).await
    }
}

/// Read-side editor-mirror state: the SUT's tracked caret byte and live
/// (pre-commit) editor text for a block. `ref_`-side id space is accepted
/// — impls resolve synthetic ids themselves (mirroring
/// `SutDriver::resolve_ref_block_id`). Binds
/// `inv-editor-caret/mirror` and `inv-editor-text/mirror`.
///
/// `#[capmap_adapter]` hosts this on `CapMap` (sync, owned `Result` returns →
/// no `#[async_trait]`; existing `E2ESut` impl untouched).
#[holon_macros::capmap_adapter]
pub trait SutEditorMirrorRead {
    /// `Err(reason)` = caret unobservable in this SUT/driver medium (the
    /// invariant reports a disclosed Skip); `Ok(None)` = observable medium
    /// but no caret tracked for this block yet.
    fn editor_caret_byte(&self, block_id: &EntityUri) -> Result<Option<usize>, String>;

    /// The live editor text for `block_id` (the `MutableText`/`InputState`
    /// value keystrokes mutate, which pre-blur can diverge from the
    /// committed block content). `Err(reason)` = unobservable in this
    /// medium / for this block right now (disclosed Skip).
    fn editor_live_text(&self, block_id: &EntityUri) -> Result<String, String>;
}

#[holon_macros::capmap_adapter]
pub trait SutFocusWrite {
    async fn apply_navigate_focus(&self, region: CapRegion, id: &EntityUri);
    async fn apply_focus_editable_text(&self, id: &EntityUri);
}

/// Navigation-history writes (`go_home`/`go_back`/`go_forward`) — distinct from
/// `SutFocusWrite` (focus → a specific block) because these traverse the
/// navigation history. `go_home` is `navigation.focus(region, None)`: it
/// *clears* current focus and the region's open pins (so it moves both current
/// focus and focus roots). `apply_navigate_back`/`apply_navigate_forward` are
/// deferred to the windowed `GpuiWindowComponent` (E4) — headless prod does not
/// yet mirror them (see `maybe_mirror_navigation_focus`), so only
/// `apply_navigate_home` lands now.
#[holon_macros::capmap_adapter]
pub trait SutNavHistoryWrite {
    async fn apply_navigate_home(&self, region: CapRegion);
}

/// Watch registration — the `setup_watch` write path decomposed off `SutHandle`
/// (SutHandle decomposition INC 3). Takes the **already-compiled** query
/// (`source` + `lang`) rather than the integration-test-local `TestQuery`,
/// which pbt-core cannot name; the `SetupWatch` transition compiles `TestQuery`
/// at the boundary (`compile_for`) and passes the result here. The read side
/// (`SutWatch`) and the watch invariants already exist and bite headlessly
/// (`frontend_slice` B5 teeth) — this is the missing **write** cap that lets a
/// composed `CapMap` drive `SetupWatch`, so the watch invariants run over a
/// composed-driven watch, not only an `E2ESut`-driven one. `&self` (like every
/// cap): the E2ESut realization is sound because its watch state is now
/// interior-mutable (`TestEnvironment::setup_watch` is `&self`).
#[holon_macros::capmap_adapter]
pub trait SutWatchRegister {
    async fn register_watch(&self, query_id: &str, source: &str, lang: holon_api::QueryLanguage);

    /// Tear down a previously-registered watch by its `query_id`. Drives the
    /// `RemoveWatch` PBT transition.
    async fn unregister_watch(&self, query_id: &str);
}

/// SUT capability: switch the active view/mode by name. Drives the `SwitchView`
/// PBT transition. Names only primitives, so it lives in `holon-pbt-core`.
#[holon_macros::capmap_adapter]
pub trait SutViewControl {
    async fn switch_view(&self, view_name: &str);
}

/// SUT capability: emit the current state over the MCP integration. Drives the
/// `EmitMcpData` PBT transition (no payload, no `ref_state`).
#[holon_macros::capmap_adapter]
pub trait SutMcpEmit {
    async fn emit_mcp_data(&self);
}

/// SUT capability: the agent-facing MCP **data tools** are drivable —
/// `dense_query` → edit → `dense_patch` round trips against a served MCP
/// surface. Drives the `DenseProjectionEdit` PBT transition (`Dense*` weight
/// family).
///
/// This is the explicit MCP-tool gate (mirrors the Loro idiom: `SutLoro` cap +
/// wiring): only compositions that actually serve MCP tools insert it — today
/// the live-MCP composition (`LiveMcpE2E`) — so every headless composition
/// deselects `Dense*` transitions naturally via cap-set narrowing. A future
/// product mode with the MCP server disabled maps onto the same axis: a
/// composition without this cap IS that mode. Deliberately NOT implied by
/// `SutMcpEmit` (which is a frontend-side emission hook, drivable headless).
#[holon_macros::capmap_adapter]
pub trait SutDenseTools {
    /// Project the children of `parent` via `dense_query`, append a new
    /// top-level headline `content` to the dense text, and apply it back via
    /// `dense_patch` — the canonical agent round trip. The patch is the SUT
    /// action; any tool error is a loud panic (never swallowed into a no-op:
    /// the ref state HAS applied the create, so a swallowed failure would
    /// surface as an unrelated block-set divergence).
    async fn dense_append_child(&self, parent: &EntityUri, content: &str);

    /// Project the children of `parent` via `dense_query`, move the FIRST
    /// top-level row to the END of the dense text (keeping its `{#alias}`
    /// token), and apply it back via `dense_patch` — the planner emits a
    /// positional `Move { after: last }` op. The patch reporting success while
    /// the row lands anywhere but last is exactly the silent-anchor-drop
    /// defect the order invariants then catch.
    async fn dense_move_first_child_to_end(&self, parent: &EntityUri);
}

/// SUT capability: undo/redo the last committed mutation. Drives the
/// `UndoLastMutation` / `Redo` PBT transitions. The block-convergence settle is
/// `ref_state`-dependent and lives in the harness seam
/// (`block_tree_post_action`), so the cap itself is a pure `&self` action over
/// the engine's undo stack.
#[holon_macros::capmap_adapter]
pub trait SutHistoryWrite {
    async fn undo_last_mutation(&self);
    async fn redo(&self);
}

/// SUT capability: drive nav-history navigation and sidebar pinning through the
/// UI driver (leader chords / synthetic dispatch). Drives the `NavigateBack`,
/// `NavigateForward`, `PinBlock`, `UnpinBlock` PBT transitions. These are
/// driver-realized only (the headless `frontend_slice` does not drive them), so
/// the cap keeps the concrete `holon_api::Region` — `pin_block` forwards the
/// region string into the dispatch params, so the lossy `CapRegion` abstraction
/// would not round-trip.
#[holon_macros::capmap_adapter]
pub trait SutNavHistoryDrive {
    async fn navigate_back(&self, region: holon_api::Region);
    async fn navigate_forward(&self, region: holon_api::Region);
    async fn pin_block(&self, region: holon_api::Region, block_id: &holon_api::EntityUri);
    async fn unpin_block(&self, history_id: i64);
    /// `OpenTabViaModifierClick`: cmd- or ctrl-click a LEFT SIDEBAR row. The
    /// sidebar `item_template` declares BOTH `cmd_action` and `ctrl_action` as
    /// `navigation_open_tab(...)` (`assets/default/index.org`), so — like
    /// `pin_block`s shift+click — the op, its region and its block id all come
    /// from the rendered template; the driver only supplies the gesture.
    async fn open_tab_via_modifier_click(&self, block_id: &holon_api::EntityUri, use_ctrl: bool);
}

/// SUT capability: block-level UI interactions driven through the UI driver —
/// clicking, drag-and-drop re-parenting, expand/collapse chevrons, the slash
/// command menu, and raw key chords. Drives the `ClickBlock`, `DragDropBlock`,
/// `ExpandToggle`, `CollapseToggle`, `TriggerSlashCommand`, `PressKey`
/// transitions. Driver-realized only (no headless `frontend_slice` driver), so
/// `E2ESut` is the sole impl; names only `holon_api` types. The `PressKey`
/// Enter-split reconciliation is `ref_state`-dependent and lives in the harness
/// seam (`block_tree_post_action`), so the cap action is a pure keystroke send.
#[holon_macros::capmap_adapter]
pub trait SutBlockInteract {
    async fn click_block(&self, region: holon_api::Region, block_id: &holon_api::EntityUri);
    async fn drag_drop_block(&self, source: &holon_api::EntityUri, target: &holon_api::EntityUri);
    async fn expand_toggle(&self, block_id: &holon_api::EntityUri);
    async fn collapse_toggle(&self, block_id: &holon_api::EntityUri);
    async fn trigger_slash_command(&self, block_id: &holon_api::EntityUri);
    async fn press_key(&self, chord: &holon_api::KeyChord);
    /// Click a rendered element by its bounds-registry id (a plain block
    /// `EntityUri` or a geometry handle `<kind>::<block-uri>`). Drives the
    /// shared `holon_layout_testing` bodies
    /// (`ToggleCollapse`/`ToggleDrawer`/ `SwitchViewMode`) via
    /// `SutClickAdapter`.
    async fn click_at_element(&self, element_id: &str);
    /// Scroll a wheel of `delta_y` pixels over the rendered element tracked
    /// under `element_id` (positive `delta_y` = scroll down). Windowed-only
    /// gesture (the `WheelScroll` transition). Native drivers synthesize a real
    /// scroll-wheel event at the element's centre; there is no headless path —
    /// `WheelScroll` is cap-gated out of every headless composition.
    async fn scroll_over(&self, element_id: &str, delta_y: f32);
}

/// Uniform quiescence abstraction. Pure slice: no-op. Wide PBT: drains
/// CDC, flushes reactive engine, awaits Loro sync.
#[holon_macros::capmap_adapter]
pub trait SutQuiesce {
    async fn quiesce(&self);
}

/// Umbrella trait for the seven T0 transitions' SUT target. Blanket-impl
/// so any `S` satisfying the four constituent traits is automatically a
/// `SutTransitionTarget`. Keeps `apply_to_sut` `where` clauses tight.
pub trait SutTransitionTarget:
    SutBlockTreeWrite + SutEditorMirrorWrite + SutFocusWrite + SutQuiesce
{
}

impl<T> SutTransitionTarget for T where
    T: ?Sized + SutBlockTreeWrite + SutEditorMirrorWrite + SutFocusWrite + SutQuiesce
{
}

// ─── Phase 6a — Loro cluster (Stage B) ───────────────────────────────
//
// Peer-Loro transitions: AddPeer, PeerEdit, PeerCharEdit, SyncWithPeer,
// MergeFromPeer, CreateStaleLoro. Surface intentionally avoids the
// integration-tests crate's `PeerEditOp` enum — the trait uses scalar +
// owned-String params so pbt-core stays dep-free of holon-api.

/// Reference-side peer-Loro read surface. Wide PBT impl delegates to
/// `ReferenceState::peers`; pure slice has no peers (returns `0`/empty).
pub trait RefPeers {
    fn peers_len(&self) -> usize;

    /// Stable IDs (peer-internal, NOT EntityUri) the peer currently holds.
    fn peer_block_stable_ids(&self, peer_idx: usize) -> Vec<String>;

    /// Stable IDs a peer has locally modified (pending, un-merged edits). The
    /// `ApplyMutation` generator excludes these from the primary's
    /// mutable-block candidate set (a block a peer is mid-editing is not a
    /// safe primary target). Wide PBT reads `peer.modified_stable_ids`; a
    /// peer-less slice returns empty.
    fn peer_modified_stable_ids(&self, peer_idx: usize) -> Vec<String>;

    /// Content of a peer's block by its stable id.
    fn peer_block_content(&self, peer_idx: usize, stable_id: &str) -> Option<String>;

    /// Parent stable id of a peer's block (None for root-level peer blocks).
    fn peer_block_parent(&self, peer_idx: usize, stable_id: &str) -> Option<String>;
}

/// Reference-side peer-Loro write surface.
pub trait RefPeersMut: RefPeers {
    /// Snapshot the primary's non-seed, non-page blocks into a new peer.
    /// Wide PBT computes the snapshot from `ReferenceState::block_state`;
    /// pure slice no-ops (returns peer_id=0).
    fn add_peer_from_primary_snapshot(&mut self) -> u64;

    fn peer_apply_create(
        &mut self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    );

    fn peer_apply_update(&mut self, peer_idx: usize, stable_id: &str, content: &str);

    fn peer_apply_delete(&mut self, peer_idx: usize, stable_id: &str);

    /// Codepoint-level insert into a peer's block content (PeerCharEdit).
    fn peer_apply_char_insert(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        text: &str,
    );

    fn peer_apply_char_delete(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        len_codepoint: usize,
    );

    /// Propagate primary's current state to peer (SyncWithPeer).
    fn peer_sync_from_primary(&mut self, peer_idx: usize);

    /// Propagate peer's pending edits back into primary (MergeFromPeer).
    fn peer_merge_into_primary(&mut self, peer_idx: usize);
}

/// Character-level text operations on a peer's LoroText container.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum TextOp {
    Insert {
        pos_codepoint: usize,
        text: String,
    },
    Delete {
        pos_codepoint: usize,
        len_codepoint: usize,
    },
}

/// Operations that can be performed on a peer's Loro tree.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum PeerEditOp {
    Create {
        parent_stable_id: Option<String>,
        content: String,
        /// Deterministic stable ID from `deterministic_peer_block_id`.
        stable_id: String,
    },
    Update {
        stable_id: String,
        content: String,
    },
    Delete {
        stable_id: String,
    },
}

/// Generate a deterministic, UUID-like stable ID from inputs.
/// Both the reference model and SUT use this to produce identical
/// block IDs for peer-created blocks (see [`PeerEditOp::Create`]).
///
/// Lives here (shared floor) rather than in the integration-test crate so the
/// co-located Loro transitions (`holon-loro-testing`) and the still-central
/// `apply_mutation` transition both reach the same implementation.
pub fn deterministic_peer_block_id(
    peer_idx: usize,
    parent_stable_id: Option<&str>,
    content: &str,
    seq: usize,
) -> String {
    use std::hash::Hash;
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    peer_idx.hash(&mut hasher);
    parent_stable_id.hash(&mut hasher);
    content.hash(&mut hasher);
    seq.hash(&mut hasher);
    let h = hasher.finish();
    let hi = (h >> 32) as u32;
    let lo = h as u32;
    format!("peer-{hi:08x}-{lo:08x}-{peer_idx:04x}-{seq:04x}")
}

/// SUT-side peer-Loro write surface. Methods are `async` because the
/// wide-PBT SUT performs real LoroDoc imports/exports + reactive-engine
/// quiescence between ops.
///
/// `&self` (not `&mut self`): the peer mesh's only structurally-mutated state
/// is the `peers` vec (one `push` in `apply_add_peer`, never across an
/// `.await`), so its provider holds it behind interior mutability. This makes
/// the trait object-safe so `#[capmap_adapter]` can host it on `CapMap` (the
/// `&self`/`Arc<dyn SutLoro>` adapter), which is what lets a composed `CapMap`
/// satisfy `SutHandle` (PCG-4).
#[holon_macros::capmap_adapter]
pub trait SutLoro {
    async fn apply_add_peer(&self);

    async fn apply_peer_create(
        &self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    );

    async fn apply_peer_update(&self, peer_idx: usize, stable_id: &str, content: &str);

    async fn apply_peer_delete(&self, peer_idx: usize, stable_id: &str);

    async fn apply_peer_char_insert(
        &self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        text: &str,
    );

    async fn apply_peer_char_delete(
        &self,
        peer_idx: usize,
        stable_id: &str,
        pos_codepoint: usize,
        len_codepoint: usize,
    );

    async fn apply_sync_with_peer(&self, peer_idx: usize);

    async fn apply_merge_from_peer(&self, peer_idx: usize);

    /// Construct a fresh peer holding a STALE snapshot (lag-N export).
    /// Wide PBT replays N pre-recorded snapshots; pure slice no-ops.
    ///
    /// Named distinctly from `SutHandle::apply_create_stale_loro` (the
    /// file-corruption variant) to avoid an ambiguous-method collision now
    /// that `SutHandle: SutLoro`.
    async fn apply_create_stale_peer(&self, lag_steps: usize);

    /// Post-startup: edit a block on a peer's LoroDoc directly.
    async fn apply_peer_edit(&self, peer_idx: usize, op: &PeerEditOp);

    /// Post-startup: edit a block's LoroText container on a peer at character
    /// level.
    async fn apply_peer_char_edit(&self, peer_idx: usize, block_id: &str, op: &TextOp);
}

/// App-runtime error log — the SUT's general "did anything error during the
/// run" surface, distinct from the component-specific error checks (the Loro
/// log in [`SutLoroLog`], the ViewModel/frontend error widgets in
/// [`SutViewSelection`]/[`SutLayout`]). Today this is the Flutter/event publish
/// errors logged during the initial document sync; `inv-no-errors` asserts the
/// count is zero. This is the home for any future non-component-specific error
/// source.
#[holon_macros::capmap_adapter]
pub trait SutErrorLog {
    /// Number of app-level error events logged since startup.
    async fn app_error_count(&self) -> usize;

    /// Identifiers (document names) present when the errors occurred — context
    /// for the failure message. Empty when there are no errors.
    async fn app_error_context(&self) -> Vec<String>;
}

/// Read-side observation of Loro state for invariants.
/// Phase 7 will bind `inv-loro-no-errors`, `inv-live-children-match-ref`
/// on this trait.
#[holon_macros::capmap_adapter]
pub trait SutLoroLog {
    /// True if the LoroSyncController logged any error since startup.
    async fn loro_had_errors(&self) -> bool;

    /// Snapshot of Loro tree children for a parent — stable-id order.
    /// `None` if the parent isn't represented in Loro.
    async fn loro_children_of(&self, parent_stable_id: &str) -> Option<Vec<String>>;

    /// The live Loro doc's Lamport height. NOT an invariant read: this is
    /// the E-solid oracle's clock-sync scalar — the shadow peer mesh pads its
    /// primary to this height at fork/sync boundaries so loro's own op-id
    /// tie-breaks ((lamport, peer)) reproduce the SUT's exactly. `None` when
    /// no live Loro doc backs this SUT (fixtures, toy SUTs).
    async fn loro_lamport_height(&self) -> Option<u32>;

    /// Every block held in the live Loro tree as typed `Block` values — the
    /// Loro store's contribution to the `inv-blocks-match-ref/loro` composite.
    /// `None` when Loro isn't enabled on this SUT (e.g. the SqlOnly variant),
    /// so the body can `Skip` rather than compare an empty store.
    async fn loro_block_snapshot(&self) -> Option<Vec<holon_api::block::Block>>;
}

// ─── Phase 6b — Turso/CDC cluster (Stage B) ──────────────────────────
//
// Binds: WriteOrgFile, BulkExternalAdd, all matview-touching invariants
// (`inv-matview-consistent-with-ref/root_layout`, `inv-watch-rows-match-ref`,
// `inv-focus-roots`, `inv-backend-blocks-match-ref` Turso side,
// `inv-sql-budget`). Required by Phase 8 storage-consistency slice.

/// SUT-side SQL projection read surface. Methods reflect Turso state
/// AFTER CDC quiescence — invariants must call `quiesce()` first.
#[holon_macros::capmap_adapter]
pub trait SutSqlProjection {
    /// Read a hydrated `block` matview row by id. `None` = row not
    /// present (deleted or never inserted). The flat Vec is the row's
    /// fields as Strings in matview-column-declaration order — concrete
    /// impls expose accessor helpers; the trait surface stays generic.
    async fn block_row(&self, id: &EntityUri) -> Option<Vec<String>>;

    /// All non-deleted block IDs visible in the projection.
    async fn all_block_ids(&self) -> BTreeSet<EntityUri>;

    /// Child block IDs of `parent` in the SQL projection, ordered by
    /// `sort_key` (the authoritative fractional index). Used by
    /// `inv-live-children-match-ref` to compare per-parent sibling order
    /// against the reference model's `RefBlockTree::sorted_children`.
    async fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri>;

    /// Row count for a watched query (used by `inv-watch-rows-match-ref`).
    async fn watch_row_count(&self, query_id: &str) -> Option<usize>;

    /// Raw block table read (no matview hydration). Used by WARN/SKIP
    /// classifier's `block_raw` truth-check.
    async fn block_raw_row(&self, id: &EntityUri) -> Option<Vec<String>>;

    /// Distinct block_id values present in the `block_tags` junction table.
    /// Used by `inv-block-tags-references-exist` to check for orphan rows
    /// (tag references whose block_id doesn't exist in block_raw).
    async fn block_tag_block_ids(&self) -> BTreeSet<EntityUri>;

    /// `task_state` JSON property for `id` from `block_raw.properties`.
    /// Returns `None` if the block doesn't exist or has no `task_state`
    /// property. Used by `inv-task-state-storage-coherence`.
    async fn block_task_state(&self, id: &EntityUri) -> Option<String>;

    /// `content` column of `block_raw` for `id`. Returns `None` if the block
    /// doesn't exist. Used by `inv-block-content/sql` (split-block
    /// content-routing slice).
    async fn block_content(&self, id: &EntityUri) -> Option<String>;
}

// `SutFocus` is the SUT side of the focus read duality, single-sourced
// with `RefFocus` by the `capability_pair! { pub trait Focus … }` above. It is
// a SEPARATE cap from `SutSqlProjection` so a storage-only slice (no navigation
// driven, e.g. `sql_slice`) does NOT register it and the focus/navigation
// invariants honestly DESELECT there instead of passing vacuously against an
// unnavigated ref. Registered only where navigation is actually driven through
// a Turso `current_focus`/`focus_roots`/`navigation_history` projection (the
// frontend slice / `full_headless`); methods reflect Turso state AFTER CDC
// quiescence.

/// SUT-side typed block-snapshot surface for `inv-backend-blocks-match-ref`.
///
/// Deliberately separate from [`SutSqlProjection`]: that trait stays
/// format-agnostic (rows as `Vec<String>`), whereas the backend-blocks
/// invariant needs the deep, per-field comparison that only typed
/// [`holon_api::Block`] values support. Coupling *this* trait to `Block`
/// keeps `SutSqlProjection`'s String surface intact.
#[allow(async_fn_in_trait)]
#[holon_macros::capmap_adapter] // emits async-trait + CapName + `impl … for CapMap`
pub trait SutBackend {
    /// Snapshot of the CDC-driven `block` matview mirror (`live_blocks`)
    /// as fully-hydrated `Block` values. Read AFTER CDC quiescence — the
    /// caller must `quiesce()` first (the wide-PBT runner does, via the
    /// shared invariant prep + convergence wait).
    async fn live_block_snapshot(&self) -> Vec<holon_api::Block>;

    /// Snapshot of the write-side `block_raw` table as `Block` values — the
    /// convergent source of truth before the IVM CDC projection. Carries only
    /// `block_raw`'s native columns (id, parent, content, content_type,
    /// source_language, properties); the junction-derived `tags`/`requires`
    /// are NOT populated, so the `inv-blocks-match-ref/block_raw` store
    /// compares a field SUBSET. Read after `quiesce()`.
    async fn block_raw_snapshot(&self) -> Vec<holon_api::Block>;

    /// Rows of the live `focus_roots` mirror (`LiveData<FocusRoot>`) as
    /// `(region, root_id)` — the CDC-driven mirror `inv-focus-roots` compares,
    /// with the `focus_roots` matview as the CDC-lag truth-check. The mirror is
    /// part of the live-CDC-mirror component alongside `live_block_snapshot`.
    async fn live_focus_root_rows(&self) -> Vec<(String, String)>;
}

/// The ordering encoding the observer's block projection carries alongside each
/// block — the `sort_key` column of the `block` matview.
///
/// A SEPARATE cap from [`SutBackend`] because the domain `holon_api::Block`
/// deliberately does NOT carry `sort_key` (ADR 0005): the fractional index is
/// the storage adapter's internal encoding, so a typed `Block` snapshot cannot
/// answer "is this block positioned?". A slice that projects no order column
/// simply does not register this, and `inv-birth-contract-satisfied` honestly
/// DESELECTS there instead of passing vacuously.
#[holon_macros::capmap_adapter]
pub trait SutOrderKeys {
    /// `(id, sort_key)` for every block the projection exposes, read after CDC
    /// quiescence. The value is the raw column string — the caller classifies
    /// it with `holon_core::fractional_index::is_minted_key`; an unkeyed
    /// sentinel (`"A0"`) is a legal column value but not a position.
    async fn live_block_order_keys(&self) -> Vec<(EntityUri, String)>;
}

/// Loro-side task_state projection. Phase 7 addition for
/// `inv-task-state-storage-coherence`. Separate from `SutLoroLog` to
/// keep the Loro-tree surface (children snapshot) isolated from the
/// property-projection surface.
#[holon_macros::capmap_adapter]
pub trait SutLoroTaskState {
    /// Task state string for `block_id` as projected from the Loro block's
    /// `properties["task_state"]` scalar — the same value the SQL sibling
    /// [`SutSqlProjection::block_task_state`] reads via
    /// `json_extract(properties,'$.task_state')`, so the two are directly
    /// comparable by `inv-task-state-storage-coherence`. `None` when Loro
    /// isn't enabled, the block is absent, or it carries no `task_state`.
    async fn loro_task_state_of(&self, block_id: &str) -> Option<String>;
}

// ─── ViewModel/Renderer cluster ──────────────────────────────────────
//
// Binds: ViewModel-touching invariants (`inv-viewmodel-*`,
// `inv-frontend-root-not-error`). Pure slice doesn't bind this.

/// Narrow viewport the value-fn provider probe forces before interpreting,
/// so the root layout picks the `if_space`-gated mobile action bar
/// (`focus_chain()` + `ops_of(...)`) on every run instead of only when the
/// generator happens to choose a chain fixture.
#[derive(Debug, Clone, Copy)]
pub struct ViewportHint {
    pub width_px: f32,
    pub height_px: f32,
}

/// Structural report on the streaming `ReactiveRowProvider`s produced by
/// value functions (`focus_chain`, `ops_of`, `chain_ops`) when the root
/// layout is interpreted. Computed SUT-side (the `ReactiveEngine` /
/// `interpret_pure` / `ProviderCache` coupling stays there) so
/// `inv-value-fn-provider-arg-variance-13` can assert purely.
/// Returned by [`SutFrontendEmissions::provider_stability_report`]; `None` from
/// that method means the root is still initializing (loading/spacer).
#[derive(Debug, Clone)]
pub struct ProviderStabilityReport {
    /// The active render_expr mentions `bottom_dock` (inv_bar precondition).
    pub mentions_bottom_dock: bool,
    /// Count of `BottomDock` nodes in the interpreted tree.
    pub bottom_dock_count: usize,
    /// The active render_expr mentions `focus_chain` (arg-variance
    /// precondition).
    pub mentions_focus_chain: bool,
    /// Total streaming providers collected in pass 1.
    pub total_providers: usize,
    /// Any pass-1 provider produced rows.
    pub any_nonempty: bool,
    /// `Some(msg)` when a `(template, rows)` group resolved to more than one
    /// `cache_identity` — provider identity instability (vfn12).
    pub identity_instability: Option<String>,
    /// Count of cache identities present in pass 1 but missing in pass 2 —
    /// provider cache flicker across re-interpret (vfn13).
    pub flicker_count: usize,
}

/// A resolved snapshot of the frontend engine's root-layout ViewModel.
/// Returned by [`SutFrontendEngine::frontend_root_vm`]; `None` from that method
/// means "no frontend engine / still loading", so any value here is a
/// settled, non-loading root.
#[derive(Debug, Clone)]
pub struct FrontendRootVm {
    /// The root widget kind (`widget_name`), e.g. `"columns"`, `"table"`.
    /// `"table"` signals the render-expr matview hasn't delivered yet (a
    /// transient loading state the bounds checks gate off of).
    pub root_kind: String,
    /// Entity ids the frontend ViewModel surfaces, in ViewModel order
    /// (`collect_entity_ids`). The geometry y-order / contiguity / coverage
    /// checks compare the rendered elements against this ordering.
    pub entity_ids: Vec<EntityUri>,
}

// ─── ViewSelection: SUT ViewModel × reference render, single-sourced ──
//
// `capability_pair!` emits BOTH read traits from one declaration:
//   - `SutViewSelection` (async, owned returns) — the SUT-side ViewModel
//     surface
//   - `RefViewSelection`    (sync, verbatim)       — the reference render-expr
//     surface
// plus the CapMap hosting glue for each, plus (for the `#[compare]` method)
// the auto-derived `inv-pair-view-selection-current-view` equality invariant
// (constructor `inv_pair_view_selection_current_view()`). The stem
// `ViewSelection` yields the trait names `SutViewSelection` /
// `RefViewSelection`.
//
// Methods are written ONCE, sync; the macro adds `async` for the SUT side.
// `drain_vm_emissions` keeps `&mut self` (a drain, not a snapshot): the CapMap
// forwarder fail-louds on it — the concrete SUT provides it in the apply phase.
holon_macros::capability_pair! {
    /// Render-expression / view-selection metadata, SUT ViewModel vs reference.
    ///
    /// The SUT side (`SutViewSelection`) is the headless `ReactiveEngine`'s rendered
    /// ViewModel surface; the reference side (`RefViewSelection`) is the production
    /// `ReferenceState`. `#[capmap_adapter]`-equivalent glue hosts both on
    /// `CapMap` (the SUT trait async → `#[async_trait(?Send)]`; the reference
    /// trait fully sync/owned → plain trait, existing `impl … for ReferenceState`
    /// untouched).
    pub trait ViewSelection {
        /// The currently selected view mode (e.g. `"all"`, `"today"`) — UI
        /// view-selection state. Auto-compared SUT-vs-reference by
        /// `inv-pair-view-selection-current-view`.
        #[compare(layer = ViewModel)]
        fn current_view(&self) -> String;

        /// Drain pending ViewModel emissions. Drain-once semantics —
        /// after drain, subsequent calls return `Vec::new` until next emit.
        /// Phase 7 `CachingProxy` memoizes this per-tick. `&mut self`, so the
        /// CapMap forwarder fail-louds — use the concrete SUT in the apply phase.
        #[sut_only]
        fn drain_vm_emissions(&mut self) -> Vec<String>;

        /// Count Error widget nodes in the headless `ReactiveEngine`'s rendered
        /// ViewModel tree. Returns `None` when the headless engine is not
        /// installed or the tree isn't ready to inspect yet (loading / placeholder
        /// / shadow-interpretation panicked). Returns `Some(n)` otherwise.
        ///
        /// `Some(0)` means "the rendered tree has no Error widgets"; the
        /// `inv-viewmodel-no-error-widgets` body asserts on that.
        #[sut_only]
        fn headless_error_node_count(&self) -> Option<usize>;

        /// Name of the active render expression for `region` (e.g. "tree",
        /// "list"). `None` when no render source block is set up yet.
        /// Wide PBT: `ReferenceState::active_render_expr_name(region)`.
        ///
        /// NOTE: this is *main-panel-preferring* — wide PBT returns
        /// `main_panel_render_expr().or(root_render_expr())`. For the
        /// `inv-viewmodel-root-matches-render-expr` check, which compares the
        /// SUT *root* widget, use `root_render_expr_name()` instead — the two
        /// diverge when a distinct main-panel render expr is set.
        #[ref_only]
        fn active_render_expr_name(&self, region: CapRegion) -> Option<String>;

        /// Function-call name of the ROOT layout's render expression
        /// specifically (NOT main-panel-preferring). `None` when no root
        /// render source block is set up, OR when the root render expr is not
        /// a `FunctionCall`. Callers distinguish those two cases via
        /// `has_root_render_expr()`. Wide PBT: the `FunctionCall { name, .. }`
        /// of `ReferenceState::root_render_expr()`.
        #[ref_only]
        fn root_render_expr_name(&self) -> Option<String>;

        /// True if the reference model has a root render expression at all.
        /// Invariants gate on this before inspecting ViewModel structure.
        #[ref_only]
        fn has_root_render_expr(&self) -> bool;

        /// Visible column names of the ROOT render expression — the column set
        /// `inv-viewmodel-decompiled-rows-match-query` filters data rows to.
        /// Wide PBT: `root_render_expr().map(|e| e.visible_columns()).unwrap_or_default()`.
        /// Empty when there's no root render expr.
        #[ref_only]
        fn root_visible_columns(&self) -> Vec<String>;

        /// Semantic id of the layout's main-panel container block, when the
        /// active layout is a multi-region layout (e.g. the 3-column layout).
        /// `None` in layout-less mode. Used by
        /// `inv-viewmodel-root-matches-render-expr` to locate the main-panel
        /// subtree in the SUT widget snapshot without hard-coding the layout's
        /// container id. Wide PBT: `ReferenceState::main_panel_block_id()`.
        #[ref_only]
        fn main_panel_block_id(&self) -> Option<EntityUri>;

        /// Function-call name of the MAIN PANEL's render expression — the content
        /// the main panel should render in a multi-region layout. Falls back to
        /// the root render expr when no distinct main-panel render expr is set.
        /// `None` when neither resolves to a `FunctionCall`. Wide PBT:
        /// `ReferenceState::main_panel_render_expr().or(root_render_expr())`'s
        /// `FunctionCall { name, .. }`.
        #[ref_only]
        fn main_panel_render_expr_name(&self) -> Option<String>;
    }
}

/// Windowed-only root-ViewModel resolution surface. A SEPARATE cap from
/// [`SutViewSelection`] because a live gpui
/// `ReactiveEngine` is the only faithful source: the headless keystone has no
/// window, so [`HeadlessFrontendComponent`] does NOT register this cap and
/// `inv-frontend-engine` / `inv-frontend-root-not-error` honestly DESELECT
/// there (this cap is a windowed-only entry on the wide cap-presence guard's
/// `WIDE_HEADLESS_ABSENT_CAPS` exclusion list) instead of "running" vacuously
/// against an honest-`None`/`false` shadow. Registered only where a live
/// window's engine is present (the windowed composition).
/// `inv-frontend-bounds-rendered` (the windowed geometry family) also reads
/// `frontend_root_vm` for the entity order its y-order / contiguity / coverage
/// checks compare against.
///
/// [`HeadlessFrontendComponent`]: (test-crate component)
#[holon_macros::capmap_adapter]
pub trait SutFrontendEngine {
    /// Resolve the FRONTEND engine's root-layout ViewModel (the gpui window's
    /// own `ReactiveEngine`) and return its root widget kind plus the ORDERED
    /// entity-id list it surfaces. `None` when the root is still loading.
    ///
    /// Read by `inv-frontend-engine` (resolution liveness) and
    /// `inv-frontend-bounds-rendered` (the entity order the geometry y-order /
    /// contiguity / coverage checks compare against).
    async fn frontend_root_vm(&self) -> Option<FrontendRootVm>;

    /// True if the frontend root ViewModel is the Error variant.
    /// Drives `inv-frontend-root-not-error`.
    async fn frontend_root_is_error(&self) -> bool;
}

/// Windowed-only ViewModel streaming-emission observer surface, a SEPARATE cap
/// from [`SutViewSelection`]. These three methods reconstruct the
/// intermediate-emission / provider-cache / live-tree behaviour of the GPUI
/// frontend directly from a live `ReactiveEngine`; a headless slice with no
/// window genuinely has no such emission surface, so
/// [`HeadlessFrontendComponent`] does NOT register this cap and the three
/// value-fn / live-tree invariants honestly DESELECT there instead of
/// "running" vacuously against honest-`None`/`[]` shadows. Registered only
/// where a live window's engine is present (the windowed composition), where
/// the checks have real teeth over the actual render pipeline.
///
/// [`HeadlessFrontendComponent`]: (test-crate component)
#[holon_macros::capmap_adapter]
pub trait SutFrontendEmissions {
    /// Force `viewport`, interpret the reactive root layout twice, and report
    /// on the streaming providers (arg variance, identity stability, cache
    /// flicker, bottom_dock presence). `None` when the root is still
    /// initializing (loading/spacer). Drives
    /// `inv-value-fn-provider-arg-variance-13`.
    async fn provider_stability_report(
        &self,
        viewport: ViewportHint,
    ) -> Option<ProviderStabilityReport>;

    /// Drain the intermediate ViewModel emissions accumulated during the last
    /// transition and extract every `StateToggle` node's `(block_id, current)`
    /// value. Drains the buffer (one-shot per tick). Drives
    /// `inv-value-fn-provider-identity`, which compares each against the
    /// reference's task state to catch CDC-enrichment glitches visible in a
    /// transient emission before a structural re-render masks them.
    async fn drain_vm_emission_toggles(&self) -> Vec<(EntityUri, String)>;

    /// Compare the persistent live ViewModel tree (the collection driver's
    /// `set_data` path, mirroring the GPUI frontend) against a freshly
    /// re-interpreted tree built from the same data rows. The fresh tree always
    /// reflects current data, so it can't catch bugs where `set_data` fails to
    /// propagate updated props to child widgets — only the live tree can.
    /// Drives `inv-live-tree-matches-fresh`.
    ///
    /// Returns:
    /// - `None` when the comparison can't run yet (no engine, root/main-panel
    ///   still loading, no rows, or no item template) — the body Skips.
    /// - `Some(vec![])` when live and fresh trees agree.
    /// - `Some(diffs)` listing the per-item prop divergences (stale props on
    ///   existing items) — the body Fails.
    async fn live_vs_fresh_tree_diff(&self) -> Option<Vec<String>>;
}

/// Frontend-agnostic widget-tree IR. The minimum surface renderer-required
/// invariants need to walk; frontends translate from their native structure
/// (e.g. `ReactiveEngine.display_tree`, real GPUI render tree) into this.
///
/// `kind`: widget type identifier matching the frontend's ViewKind tag
/// ("editable_text", "draggable", "state_toggle", "live_block", etc.).
/// `entity_id`: the block / row id this widget renders, if any.
/// `props`: scalar widget properties as canonical strings — e.g. for a
/// `state_toggle`, this carries `field`, `current`, `label`, `states` as
/// JSON-encoded values. Invariants parse from this map; the contract is
/// "frontend serializes props it wants checked, in stable canonical form."
/// `operations`: bound operations as canonical strings, one per op, of
/// the shape `<op_name>:<key>:<value>` (e.g. `set_field:task_state:DONE`).
/// Invariants match by prefix.
/// `children`: nested widgets in render order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetSnapshot {
    pub kind: String,
    pub entity_id: Option<String>,
    pub props: std::collections::BTreeMap<String, String>,
    pub operations: Vec<String>,
    pub children: Vec<WidgetSnapshot>,
}

impl WidgetSnapshot {
    /// Pre-order recursive iterator over self + descendants.
    pub fn walk(&self) -> WidgetSnapshotIter<'_> {
        WidgetSnapshotIter { stack: vec![self] }
    }

    /// First operation whose canonical string starts with `prefix`.
    pub fn find_op(&self, prefix: &str) -> Option<&str> {
        self.operations
            .iter()
            .find(|op| op.starts_with(prefix))
            .map(String::as_str)
    }

    /// All non-None `entity_id` values reachable in the tree, deduped.
    /// `live_block` widgets carry the referenced block id as `entity_id`
    /// per the translator contract.
    pub fn collect_entity_ids(&self) -> BTreeSet<String> {
        self.walk().filter_map(|n| n.entity_id.clone()).collect()
    }

    /// Whether this node is a display-placed occurrence (ADR 0015 P2). The
    /// production `view_model_to_snapshot` stamps `Occurrence::Placed` rows
    /// with `props["occurrence"]`; non-placed rows carry no such prop. This
    /// is typed detection, never an id-infix sniff.
    pub fn is_display_placed(&self) -> bool {
        self.props.contains_key("occurrence")
    }

    /// All non-None `entity_id` values reachable in the tree, deduped,
    /// EXCLUDING nodes whose `is_display_placed()` returns true.
    /// Display-placed rows carry a canonical entity id but are
    /// display-only; invariants that judge entity-id membership (e.g.
    /// `inv-main-panel-rows-match-focus`) must skip them to avoid flagging
    /// a display-only occurrence as a "stale row."
    pub fn collect_canonical_entity_ids(&self) -> BTreeSet<String> {
        self.walk()
            .filter(|n| !n.is_display_placed())
            .filter_map(|n| n.entity_id.clone())
            .collect()
    }

    /// All nodes whose `kind` equals `kind`.
    pub fn collect_by_kind<'a>(&'a self, kind: &str) -> Vec<&'a WidgetSnapshot> {
        self.walk().filter(|n| n.kind == kind).collect()
    }

    /// All `entity_id` values of nodes whose `kind` matches any of `kinds`.
    pub fn entity_ids_of_kinds(&self, kinds: &[&str]) -> BTreeSet<String> {
        self.walk()
            .filter(|n| kinds.iter().any(|k| n.kind == *k))
            .filter_map(|n| n.entity_id.clone())
            .collect()
    }
}

/// Pre-order traversal iterator over a `WidgetSnapshot` tree.
pub struct WidgetSnapshotIter<'a> {
    stack: Vec<&'a WidgetSnapshot>,
}

impl<'a> Iterator for WidgetSnapshotIter<'a> {
    type Item = &'a WidgetSnapshot;
    fn next(&mut self) -> Option<&'a WidgetSnapshot> {
        let node = self.stack.pop()?;
        for child in node.children.iter().rev() {
            self.stack.push(child);
        }
        Some(node)
    }
}

#[holon_macros::capmap_adapter]
pub trait SutRenderer {
    /// Stringified render-tree for a block id (debug-formatted).
    /// Used by `inv-displayed-text` and OrgRender fixed-point checks.
    async fn render_tree_of(&self, id: &EntityUri) -> Option<String>;

    /// Frontend-agnostic snapshot of the current widget tree. Any slice
    /// with a renderer (wide PBT, hypothetical Phase 9 in-memory + GPUI)
    /// can produce one; pure / storage-only slices have no widget tree
    /// and don't implement this trait at all.
    ///
    /// Returns the root widget; descendants reachable via `.children`.
    async fn widget_tree_snapshot(&self) -> WidgetSnapshot;

    /// Non-cached companion to [`Self::widget_tree_snapshot`] for
    /// bounded-wait / poll-style invariants that must RE-SAMPLE a
    /// self-healing transient frame within one check tick — e.g.
    /// `inv-embedded-page-collapsed-lazy` waiting out the
    /// `focus_descendants` recursive-CTE prune delta that lands a frame
    /// after the STRICT one-shot BlockToPage snapshot.
    ///
    /// The cached `widget_tree_snapshot` freezes ONE recompute per tick
    /// (the `CachingProxy` per-tick memo AND the composed
    /// `HeadlessFrontendComponent` armed `render_snapshot_cache`), so a
    /// wait loop calling it can never observe the heal. This method
    /// bypasses those memos, recomputes, and refreshes the memo so a
    /// later same-tick consumer sees the newest frame — the exact
    /// discipline `SutLayout::rendered_elements_fresh` uses for the
    /// geometry poll invariants. SUTs with no caching forward to
    /// `widget_tree_snapshot`.
    async fn widget_tree_snapshot_fresh(&self) -> WidgetSnapshot;

    /// Block ids in the data_rows that feed the current root layout's
    /// widget tree — i.e. what the renderer is reading from query
    /// results. Used by `inv-viewmodel-entity-ids-subset-of-data` to
    /// assert tree-referenced entity_ids are a subset of available
    /// data rows.
    async fn root_data_row_ids(&self) -> BTreeSet<EntityUri>;

    /// Widget tree for a SPECIFIC block id — the snapshot the renderer
    /// would produce if that block were the root of its own subtree.
    /// Used by invariants that need per-block-subtree BFS (e.g.
    /// `inv-editable-text-has-draggable`, which enforces pairing within
    /// each block_profile-rendered tree independently).
    ///
    /// Returns `None` if `block_id` doesn't resolve (no such block /
    /// not watchable yet). A "live_block" node referenced inside
    /// another tree's snapshot is the typical input: caller BFS-es by
    /// following live_block children, calling this method per discovered
    /// block id.
    async fn widget_tree_for(&self, block_id: &EntityUri) -> Option<WidgetSnapshot>;

    /// "Decompiler" content comparison for the root layout, used by
    /// `inv-viewmodel-decompiled-rows-match-query`.
    ///
    /// Interprets the root layout's render_expr against its data_rows into
    /// a display tree, extracts the per-row rendered content strings
    /// ("decompiled" inverse of the renderer), and pairs them with the
    /// `content` column of the underlying query `data_rows` filtered to the
    /// reference render expr's `visible_columns` (passed in `visible_columns`).
    ///
    /// Returns `Some((rendered_content, data_content))` — two `content`
    /// string vectors the body compares via an ordered-subset check
    /// (`rendered ⊆ data`, in order).
    ///
    /// Returns `None` when the comparison must not run — i.e. the root
    /// isn't ready (loading / spacer / not watchable), or any of the inline
    /// gates is empty (`rendered_rows`, `visible_columns`, or `data_rows`).
    /// The body treats `None` as `Ok`.
    async fn root_content_comparison(
        &self,
        visible_columns: &[String],
    ) -> Option<(Vec<String>, Vec<String>)>;

    /// Readiness signal for the root render.
    ///
    /// `true` iff the root layout's render expression is a real content
    /// expression (NOT the `loading` placeholder, NOT a `spacer`
    /// placeholder) AND the headless interpretation of it succeeds. This
    /// mirrors the inline `inv-viewmodel-snapshot` block's guards (skip on
    /// closed stream / `loading` / `spacer` / interpret panic): structural
    /// ViewModel assertions whose contract only holds for a settled content
    /// render must consult this first and skip when it returns `false`,
    /// rather than asserting against a transient placeholder root.
    async fn root_render_ready(&self) -> bool;

    /// The root `RenderExpr` `FunctionCall` name from the resolved watch
    /// snapshot's render-expr side — e.g. `"table"`, `"columns"`, or
    /// `"source_editor"` (the degraded no-query-engine render). `None` when the
    /// root is not ready: no watch resolved, a `loading`/`spacer` placeholder,
    /// or the render expr is not a `FunctionCall`. The degraded twin
    /// `inv-viewmodel-shows-source-when-no-query` reads this; `None` → it
    /// Skips.
    async fn root_render_kind(&self) -> Option<String>;
}

/// SUT-side query-results read surface. Present only when a real query engine
/// backs the render path (the Turso `BackendEngine` + `ReactiveEngine`
/// `watch_query_live` surface the full-mode frontend component owns). A
/// degraded no-Turso ("shows source") frontend has NO query engine and so does
/// NOT provide this cap — making it the negative-selection (`sut_absent`)
/// discriminator between the full-mode
/// `inv-viewmodel-decompiled-rows-match-query` twin and the degraded
/// `inv-viewmodel-shows-source-when-no-query` twin.
#[holon_macros::capmap_adapter]
pub trait SutQueryResults {
    /// Number of rows the root layout's watch query produced. `None` when the
    /// root watch is not ready (no query engine, or still loading).
    async fn root_query_row_count(&self) -> Option<usize>;
}

// ─── Phase 6d — Layout/Bounds cluster ────────────────────────────────
//
// Re-export trait over `holon_pbt_core::user_driver::UserDriver` geometry
// methods. Phase 7 binds `inv-frontend-bounds-*`,
// `inv-editable-text-has-draggable`, `inv-frontend-no-error-widgets`.

/// One element from the rendered window's geometry registry — the
/// pbt-core-side mirror of `holon_frontend::geometry::ElementInfo`, so
/// `holon-pbt-core` carries no `holon-frontend` dependency.
///
/// Verdicts that depend on `holon-frontend`-only logic are computed on the
/// SUT side and stored here, keeping the invariant bodies pure:
/// - `expected_size_violation` is the result of
///   `ElementInfo::expected_size.check(..)` evaluated against the full element
///   snapshot (`ProviderEvalCtx`).
/// - `is_error_widget` is `widget_type == "error"`.
#[derive(Debug, Clone)]
pub struct RenderedElement {
    /// Registry element id (e.g. `render-entity-block:…`, `editable-text-…`).
    pub el_id: String,
    /// Widget kind: `"editable_text"`, `"rendered_text"`, `"text"`,
    /// `"draggable"`, `"error"`, container kinds, …
    pub widget_type: String,
    /// The block this element is data-bound to, if any. Already in SUT id
    /// space (real UUIDs) — directly comparable to the runner's resolved ref.
    pub entity_id: Option<EntityUri>,
    /// The string actually on screen (live `InputState` value for
    /// `editable_text`, resolved prop for `text`). `None` for containers.
    pub displayed_text: Option<String>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// False for empty containers.
    pub has_content: bool,
    /// Immediate tracked parent's el_id, `None` at the tracked-tree root.
    pub parent_id: Option<String>,
    /// `Some(violation)` when the observed `(w, h)` fails the element's
    /// declared `expected_size`; `None` when satisfied/unconstrained.
    pub expected_size_violation: Option<String>,
    /// `widget_type == "error"` — surfaced so bodies need no widget-type
    /// string match.
    pub is_error_widget: bool,
    /// Whether this widget's focus handle held WINDOW focus when the frame
    /// was committed. `None` for widgets without a focus handle. Engine
    /// `focused_block` moves synchronously; window focus follows via a
    /// spawned binding — the divergence window is exactly the
    /// steal-back/zombie-editor bug family.
    pub focused: Option<bool>,
    /// The read-mode styled-run fingerprint this element actually painted for
    /// its block (byte-range runs, theme-independent), or `None` when it
    /// painted plain text / carries no marks. `inv-paint-text-styling`
    /// compares this against the fingerprint the block's `(content, marks)`
    /// demand — a marked block that painted plain shows up here as
    /// `None`/empty.
    pub styled_runs: Option<Vec<holon_api::StyledRun>>,
}

/// Windowed frontends whose production mount path wraps each PANEL block
/// (`block:default-*`) in its own `ReactiveShell` entity and tracks that
/// wrapper in the geometry registry. GPUI does this
/// (`render/builders/live_block.rs`); the TUI deliberately does not, so it
/// registers [`SutInlineRowMount`] instead and the shell-wrapper invariant
/// honestly deselects there rather than failing on an observable that
/// frontend never emits.
#[holon_macros::capmap_adapter]
pub trait SutPerBlockShellMount {
    /// The `widget_type` the shell wrapper is tagged with in the registry.
    fn panel_shell_widget_type(&self) -> String;
}

/// Windowed frontends whose production mount path resolves each doc-block row
/// inline, leaving no per-block shell wrapper in the geometry registry — rows
/// are tracked directly as the rendering of their entity. The TUI does this
/// (`render/mod.rs`, the `tree`/`table`/`outline` collection boundary); it is
/// the mount-faithfulness counterpart to [`SutPerBlockShellMount`].
#[holon_macros::capmap_adapter]
pub trait SutInlineRowMount {
    /// The `widget_type` an inline-resolved doc-block row is tagged with.
    fn inline_row_widget_type(&self) -> String;
}

#[holon_macros::capmap_adapter]
pub trait SutLayout {
    /// Snapshot every tracked element in the rendered window's geometry
    /// registry. Empty when no geometry provider is installed (headless
    /// variants) — the `[FrontendBounds]` invariants the registry selects
    /// only for the gpui suite treat an empty snapshot as `Skipped`.
    ///
    /// The single component-snapshot that `inv-frontend-bounds-rendered`,
    /// `inv-displayed-text`, and `inv-frontend-engine` read (mirrors the
    /// block-store `*_snapshot()` pattern). SUT-computed verdicts ride along
    /// on each [`RenderedElement`] so the bodies stay pure.
    async fn rendered_elements(&self) -> Vec<RenderedElement>;

    /// Uncached variant of [`Self::rendered_elements`]: always re-reads the
    /// live geometry registry, and implementations should pump a frame first
    /// when they can (an occluded GPUI window commits no frames on its own,
    /// so reads would otherwise stay frozen on the last committed pass).
    ///
    /// Poll-style invariants MUST use this: the per-tick `CachingProxy`
    /// memoises `rendered_elements`, so a retry loop polling the cached
    /// method observes the same frozen snapshot on every iteration and a
    /// transient lag (e.g. window focus trailing the engine by a frame or
    /// two) becomes a guaranteed "settled" failure.
    async fn rendered_elements_fresh(&self) -> Vec<RenderedElement> {
        self.rendered_elements().await
    }

    /// Fraction of content-area pixels (below the title bar) that differ from
    /// the background in the most recent window screenshot — the pixel-level
    /// ground truth for `inv-frontend-bounds-rendered`'s `not-visually-empty`
    /// backstop. `None` when no screenshot watcher is installed or no frame
    /// has been analysed yet. Near-0 means a blank window.
    async fn visual_content_fraction(&self) -> Option<f32>;

    /// True if a widget for `id` is currently registered with bounds.
    async fn has_registered_bounds(&self, id: &EntityUri) -> bool;

    /// True if a draggable handle is wired for `id`.
    async fn has_draggable_handle(&self, id: &EntityUri) -> bool;

    /// True if any rendered widget is an Error variant.
    async fn any_error_widget(&self) -> bool;

    /// Wait until a widget for `id` is registered in BoundsRegistry, or
    /// `timeout` elapses. Returns `Err(diagnostic_string)` on timeout —
    /// callers panic for input-bearing transitions per fail-loud policy.
    /// Implementations may issue a scroll-into-view RPC if the bounds are
    /// missing (virtualized lists do not prepaint offscreen rows).
    async fn wait_for_bounds(&self, id: &EntityUri, timeout: Duration) -> Result<(), String>;

    /// Wait until the widget rendered at `id` matches one of `accepted`
    /// kinds (e.g. `["editable_text", "rendered_text"]`), or `timeout`
    /// elapses. Stronger precondition than `wait_for_bounds`: confirms
    /// the click target is the *interactive* variant the transition
    /// expects, not just any element carrying the entity_id.
    ///
    /// Returns `Ok(())` when no geometry is installed (headless variants
    /// don't need widget-kind gating).
    async fn wait_for_widget_kind(
        &self,
        id: &EntityUri,
        accepted: &[&str],
        timeout: Duration,
    ) -> Result<(), String>;

    /// Wait until `id`'s `editable_text` widget reports it holds WINDOW
    /// focus (`ElementInfo::focused == Some(true)`), or `timeout` elapses.
    /// Engine focus moves synchronously; window focus follows a spawned
    /// binding — keystrokes dispatched before it lands are consumed by the
    /// previously-focused editor. Returns `Ok(())` when no geometry is
    /// installed (headless variants dispatch synchronously).
    async fn wait_for_window_focused_editor(
        &self,
        id: &EntityUri,
        timeout: Duration,
    ) -> Result<(), String>;
}

// ─── Phase 6e — Driver cluster ───────────────────────────────────────
//
// Re-export of `UserDriver` input methods. Phase 7 binds
// `inv-focus-matches-ref`. Driver methods are already trait-bound;
// this re-export keeps slice opt-in symmetric with the other clusters.

/// Frontend-engine focus, with "no engine installed" kept distinct from
/// "engine has no focus". Conflating the two (an `Option<EntityUri>` `None`)
/// made the focus steal-back bug family read as green: a lost focus looked
/// identical to SqlOnly mode and was skipped instead of failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineFocus {
    /// No frontend engine is installed (SqlOnly headless). Focus is
    /// unobservable here — checks must report `Skipped`, not `Ok`.
    NoEngine,
    /// An engine is installed but `focused_block()` is `None`. A real,
    /// comparable state: fails when the ref expects focus.
    Unfocused,
    Focused(EntityUri),
}

#[holon_macros::capmap_adapter] // mixed trait (7 async + 1 sync) → async-trait(?Send); emits CapName + `impl …
// for CapMap`
pub trait SutDriver {
    async fn driver_send_key_chord(&self, chord: &str);
    async fn driver_click(&self, id: &EntityUri);
    /// Region-aware click. Mirrors `UserDriver::click_entity(entity_id,
    /// region)` (`region` is "main", "left_sidebar", ...). `driver_click`
    /// is the region-defaulted convenience wrapper that panics on error;
    /// `click_entity` returns the result so callers can attach their own
    /// transition-specific diagnostic.
    async fn click_entity(&self, id: &EntityUri, region: &str) -> Result<(), String>;
    /// Poll until `engine_focused_block` returns `Some(id)` or `timeout`
    /// elapses. Used as a post-click barrier — GPUI's mouse-click goes
    /// through `dispatch_intent` (fire-and-forget), so subsequent
    /// transitions need an explicit gate before they read focus.
    async fn wait_for_engine_focus(&self, id: &EntityUri, timeout: Duration) -> Result<(), String>;
    /// Send a single raw key with modifiers. `key` is a key name like
    /// `"home"`, `"right"`, `"enter"`, `"backspace"`, or a single
    /// character (`"a"`). `modifiers` is a slice of `"cmd"`, `"ctrl"`,
    /// `"alt"`, `"shift"`. Mirrors `UserDriver::send_raw_keystroke`.
    async fn send_raw_keystroke(&self, key: &str, modifiers: &[&str]) -> Result<(), String>;
    async fn driver_current_focus(&self) -> Option<EntityUri>;
    /// The globally focused block id as tracked by the reactive/frontend
    /// engine (distinct from the per-region SQL `current_focus` matview).
    /// Set by click handlers; read by `inv-focus-matches-ref`.
    /// `NoEngine` when no frontend engine is installed (SqlOnly mode) —
    /// kept distinct from `Unfocused` so lost focus fails instead of skips.
    async fn engine_focused_block(&self) -> EngineFocus;
    /// Translate a reference-model block id (which may be a synthetic URI
    /// like `block:ref-doc-0`) to the resolved UUID-based id that the SUT
    /// engine tracks. Wide PBT: delegates to `E2ESut::resolve_uri` via
    /// `doc_uri_map`; pure slice: returns the id unchanged (no synthetic URIs).
    fn resolve_ref_block_id(&self, id: &EntityUri) -> EntityUri;
}

// ─── Phase 6f — OrgRender cluster ────────────────────────────────────
//
// Binds: `inv-org-render-fixed-point`.

#[holon_macros::capmap_adapter] // emits async-trait + CapName + `impl … for CapMap`
pub trait SutOrgRender {
    /// Snapshot every tracked org file as `(path, disk_text, rendered_text)`
    /// where `disk_text` is the bytes currently on disk and `rendered_text`
    /// is what the renderer would emit from the current SQL state.
    ///
    /// Used by `inv-org-render-fixed-point` to assert `disk == rendered`
    /// — required so the echo-suppression loop in `re_render_all_tracked`
    /// doesn't spin on a permanent disagreement.
    async fn snapshot_org_render_pairs(&self) -> Vec<(String, String, String)>;
}

// ─── Phase 6f' — OrgRead cluster ─────────────────────────────────────
//
// Binds: `inv-blocks-match-ref/org`. The org-file store in the
// block-equivalence composite — distinct from `SutOrgRender`, which reads
// the render-vs-disk fixed point. This one parses the on-disk org files
// back into blocks so they can be compared against the reference.

#[holon_macros::capmap_adapter]
pub trait SutOrgRead {
    /// Wait for the FileSyncController's background re-render to settle, then
    /// parse every tracked org file on disk back into `holon_api::Block`s.
    ///
    /// Folds the monolith's `wait_for_org_files_stable` +
    /// `parse_org_file_blocks` into one snapshot, mirroring the other
    /// block-store snapshot caps ([`SutBackend::block_raw_snapshot`],
    /// [`SutLoroLog::loro_block_snapshot`]). The org parser produces
    /// `block:<uuid>` parents for `#+ID:`-resolved docs
    /// and `file:<filename>` parents for unresolved ones — the reference side
    /// (`RefBackend::org_blocks`) mirrors that same parent resolution.
    async fn org_block_snapshot(&self) -> Vec<holon_api::Block>;
}

// ─── Phase 6f'' — FsWrites cluster ───────────────────────────────────
//
// Binds: `inv-no-write-outside-vault-root`.

#[holon_macros::capmap_adapter]
pub trait SutFsWrites {
    /// `(vault root, every path the filesystem was ASKED to write or create)`,
    /// both normalized. The write log — not a directory listing — because a
    /// target that escaped the root is a defect even if the write then failed
    /// or the file was removed again.
    async fn vault_write_targets(&self) -> (String, Vec<String>);
}

// ─── Reference-side: extended read-only projections ──────────────────
//
// Each surfaces `ReferenceState` fields that invariant bodies need. Thin
// read-only projections; the blanket impl in `reference_capabilities.rs`
// delegates directly to the corresponding field/method on `ReferenceState`.

/// Focus-roots expected by the reference model — per-region set of
/// block ids that the reactive engine should use as pin roots.
pub trait RefFocusRoots {
    /// The RENDERED focus root for `region` — the cursor's open row, i.e.
    /// what the main panel projects. NOT the region's open SET (background
    /// tabs are open but unrendered): that is `expected_focus_root_rows`.
    /// Wide PBT reads `ReferenceState::rendered_focus_root`; pure slice: empty.
    fn rendered_focus_root_ids(&self, region: CapRegion) -> BTreeSet<EntityUri>;
}

/// Layout-block metadata needed by matview + ViewModel invariants.
#[holon_macros::capmap_adapter] // sync trait → no async-trait; emits CapName + `impl … for CapMap`
pub trait RefLayout {
    /// All block ids that are part of the layout scaffolding (headline,
    /// query-source, render-source). `is_layout_block` on `RefBlockTree`
    /// is the per-id predicate; this gives the full set for iteration.
    fn layout_block_ids(&self) -> BTreeSet<EntityUri>;

    /// Block ids of the active profile blocks (from `profile_block_ids`).
    fn profile_block_ids(&self) -> BTreeSet<EntityUri>;

    /// True if the test has an active "block" profile override.
    /// Wide PBT: `ReferenceState::has_blocks_profile()`; pure slice: `false`.
    fn has_blocks_profile(&self) -> bool;

    /// True when a user-provided `index.org` custom layout is active — the
    /// `CreateBlockUnderFocus` slot-parenting guarantee only holds under the
    /// default layout. Wide PBT: `ReferenceState::has_user_index_org()`.
    fn has_user_index_org(&self) -> bool;

    /// Every block id the reference model tracks, **including** seed and
    /// source blocks. Unlike `RefBlockTree::all_non_seed_block_ids`, this
    /// keeps seed blocks so the matview-consistency invariant can build
    /// the full "known to the DB" set without false `extra` reports.
    /// Wide PBT: keys of `block_state.blocks`; pure slice: empty.
    fn all_block_ids(&self) -> BTreeSet<EntityUri>;

    /// The blocks the reactive root layout is *expected* to surface for
    /// `region`: every block that descends from the region's expected focus
    /// roots. Source blocks are included — per the fork-A program-rendering
    /// ruling they render as visible rows (rule cards / query results), not
    /// hidden; staleness is a cross-root property, so the descendant gate alone
    /// defines the allowed set. Used by `inv-main-panel-rows-match-focus` to
    /// detect rows from a *previous* focus root lingering after navigation.
    /// Wide PBT filters `block_state.blocks` by
    /// `is_descendant_of_any(expected_focus_root_ids(region))`; pure slice:
    /// empty.
    fn expected_visible_content_ids(&self, region: CapRegion) -> BTreeSet<EntityUri>;

    /// True when the reference model has at least one user document. Gates the
    /// `inv-frontend-bounds-rendered` content checks (non-wrapper-content,
    /// not-visually-empty) that only make sense once docs exist. Wide PBT:
    /// `!ReferenceState::documents.is_empty()`; pure slice: `false`.
    fn has_user_documents(&self) -> bool;

    /// True when an entity is click/arrow-focused in `region` (the
    /// `focused_entity_id` map, distinct from navigation history). Used by the
    /// `not-visually-empty` backstop to pick the stricter content threshold
    /// for a focused main panel. Wide PBT:
    /// `ReferenceState::focused_entity_id.contains_key(region)`; pure slice:
    /// `false`.
    fn region_entity_focused(&self, region: CapRegion) -> bool;
}

/// Reference-side open-pin READ surface (`UnpinBlock`'s generator +
/// precondition). Returns plain ids / bools — never the integration-test-only
/// open-pin row type (`OpenPinEntry`) — so `holon-pbt-core` stays decoupled
/// from the harness row layout. Region-precise focus lookups stay inside the
/// impl.
pub trait RefPins {
    /// `history_id` of every open pin across all regions — the `UnpinBlock`
    /// candidate set (both closeable and not; `is_closeable_pin` narrows it).
    fn open_pin_history_ids(&self) -> Vec<i64>;
    /// True iff an open pin with `history_id` exists whose `block_id` is
    /// non-null AND is not its region's current cursor focus — the
    /// X-button-closeable predicate. The active cursor focus is closed by
    /// navigating away, never by the X button, so closing it via
    /// `navigation.close` is a no-op the ref must not predict.
    fn is_closeable_pin(&self, history_id: i64) -> bool;
}

/// Reference-side open-pin mutation surface — the LogSeq shift-click
/// (`navigation.focus_pin`) / X-button (`navigation.close`) semantics the
/// `PinBlock` / `UnpinBlock` transitions model.
///
/// Each mutation encapsulates its whole update/insert/delete bookkeeping so the
/// integration-test-only open-pin row type (`OpenPinEntry`) never has to cross
/// into `holon-pbt-core`.
pub trait RefPinsMut: RefPins {
    /// Pin `block_id` in `region` with move-to-top semantics, mirroring
    /// `provider.rs::focus_pin`: if an open pin already exists for
    /// `(region, block_id)`, bump its logical timestamp in place (UPDATE — no
    /// new row); otherwise mint a fresh open-pin row (INSERT), advancing both
    /// the pin-timestamp and the history-id counters. Wide PBT mutates
    /// `ui.user.open_pins` / `ui.user.next_pin_ts` / `ui.tab.next_history_id`;
    /// a pin-less pure slice no-ops.
    fn upsert_open_pin(&mut self, region: holon_api::Region, block_id: &EntityUri);
    /// `UnpinBlock`: close (remove) the open pin with `history_id` from every
    /// region's open-pin set. Mirrors `navigation.close`'s `closed_at` UPDATE.
    fn close_pin(&mut self, history_id: i64);
}

/// Reference-side navigation-history read surface — the per-region back/forward
/// stack plus the sidebar-navigation prediction gates the `Navigate*`
/// transitions read. Region-precise (`holon_api::Region`, not the lossy
/// `CapRegion`) because the nav history is keyed per exact region.
pub trait RefNavHistory {
    /// True iff the region's history cursor can move back (has a prior entry).
    fn can_go_back(&self, region: holon_api::Region) -> bool;
    /// True iff the region's history cursor can move forward.
    fn can_go_forward(&self, region: holon_api::Region) -> bool;
    /// True iff production would bind `navigation.focus(region)` on `block_id`
    /// (the default sidebar's rendered doc list). Pure predicate over ref
    /// state.
    fn predicts_navigation_focus(&self, block_id: &EntityUri, region: holon_api::Region) -> bool;
    /// The page blocks the default sidebar renders as nav-focus targets.
    fn predicted_sidebar_navigation_targets(&self) -> Vec<EntityUri>;
    /// True iff the drawer panel `panel_id` is open — sidebar clicks only reach
    /// their targets while the panel is expanded. Defaults open when untracked.
    fn drawer_is_open(&self, panel_id: &str) -> bool;
}

/// Reference-side navigation-history + focus mutation surface for the
/// `Navigate*` transitions. Each method is the WHOLE per-transition reference
/// effect (cursor move / history push / open-pin reset / region-focus clear /
/// editor blur / global-focus update), encapsulated so the
/// integration-test-only `OpenPinEntry` / `NavigationHistory` row types never
/// cross into `holon-pbt-core`.
pub trait RefNavHistoryMut: RefNavHistory {
    /// `NavigateBack`: step the region cursor back one entry (if possible),
    /// clear the region's per-block focus, and blur the active editor.
    fn nav_step_back(&mut self, region: holon_api::Region);
    /// `NavigateForward`: symmetric forward cursor step + focus clear + blur.
    fn nav_step_forward(&mut self, region: holon_api::Region);
    /// `NavigateHome`: `focus(region, None)` — push a home (NULL) history row
    /// and reset the region's open pins to that single home row (idempotent
    /// when already home), clear region + global focus, blur.
    fn nav_go_home(&mut self, region: holon_api::Region);
    /// `NavigateFocus`: `focus(region, block_id)` — push a focus history row
    /// and reset the region's open pins to that single focus row
    /// (idempotent when already focused there), record the first-visit
    /// budget flag, set the block as global focus, clear region focus,
    /// blur.
    fn nav_focus(&mut self, region: holon_api::Region, block_id: &EntityUri);
    /// `OpenTabViaModifierClick`: `open_tab(region, block_id)` — APPEND an open
    /// row for `block_id` if the region has none (never closing or
    /// reordering the others, unlike `nav_focus`), then move the cursor
    /// onto it. When a row is already open for that block, prod's
    /// `open_tab` delegates to `activate`: cursor-only, no new row. The
    /// appended row is a BACKGROUND tab until the cursor lands on it — open
    /// in `focus_roots`, unrendered by the panel.
    fn nav_open_tab(&mut self, region: holon_api::Region, block_id: &EntityUri);
}

/// Reference-side document read surface — the `files.documents` map (uri →
/// filename) that the document + boot transitions query. Returns names / uris /
/// counts / bools; never the integration-test-only map type.
pub trait RefDocuments {
    /// Every tracked document filename (values of `files.documents`).
    fn document_names(&self) -> Vec<String>;
    /// True iff a document with this exact filename is tracked.
    fn has_document(&self, file_name: &str) -> bool;
    /// Number of tracked documents (`files.documents.len()`).
    fn document_count(&self) -> usize;
    /// Resolve a document's uri from its NAME (file stem), if tracked.
    fn doc_uri_by_name(&self, name: &str) -> Option<EntityUri>;
    /// The document uri a block currently belongs to (`block_documents[id]`),
    /// if any.
    fn block_document_of(&self, block_id: &EntityUri) -> Option<EntityUri>;
    /// True iff the reference holds a NON-SEED advice-rule block — the
    /// ≤1-active-rule gate `WriteOrgFile` consults before seeding another
    /// rule.
    fn has_non_seed_advice_rule(&self) -> bool;
    /// Every tracked document uri (keys of `files.documents`) —
    /// `BulkExternalAdd`'s candidate-doc set.
    fn document_uris(&self) -> Vec<EntityUri>;
    /// True iff `uri` is a tracked document.
    fn has_document_uri(&self, uri: &EntityUri) -> bool;
}

/// Reference-side document mutation surface. Each method encapsulates the whole
/// per-transition reference effect (page-block creation, block-tree surgery,
/// canonical re-sequencing, profile rebuild) so integration-test-only
/// block/layout internals never cross into `holon-pbt-core`.
pub trait RefDocumentsMut: RefDocuments {
    /// `CreateDocument`: mint a synthetic doc uri, register the filename, and
    /// insert the empty page block. Advances the synthetic-doc counter.
    fn insert_document(&mut self, file_name: &str);
    /// `DeleteDocument`: remove the document and cascade-delete its page block
    /// and all descendants, re-canonicalizing sibling order and clearing
    /// dangling focus.
    fn remove_document(&mut self, file_name: &str);
    /// `RenameDocument`: retitle the document tracked as `old_file_name` to
    /// the stem of `new_file_name` and re-point its filename. The file-move
    /// spec: a page's title FOLLOWS its file name, so the reference title
    /// tracks the new stem — which production now matches via the atomic Rename
    /// port (`FileSyncController::on_file_renamed` re-homes + retitles).
    fn rename_document(&mut self, old_file_name: &str, new_file_name: &str);
    /// `WriteOrgFile`: (re)seed a document's blocks from generator-produced
    /// `Block`s before startup — remap placeholder parents, normalize the
    /// org round-trip, classify index-layout source blocks,
    /// re-canonicalize, and advance the pre-startup file counter.
    /// `todo_keywords` is the file's `#+TODO:` set (adopted by the document
    /// block).
    fn seed_org_file(
        &mut self,
        filename: &str,
        blocks: &[holon_api::block::Block],
        todo_keywords: Option<Vec<holon_api::TaskState>>,
    );
}

/// Reference-side pre-startup boot read surface — the fixture counters and
/// VCS-init flags the boot / fixture transitions gate on.
pub trait RefBoot {
    /// Number of directories staged pre-startup
    /// (`pre_startup_directories.len()`).
    fn pre_startup_directory_count(&self) -> usize;
    /// Number of org files written pre-startup (`pre_startup_file_count`).
    fn pre_startup_file_count(&self) -> usize;
    /// Whether a git repo has been initialized in the fixture.
    fn git_initialized(&self) -> bool;
    /// Whether a jj repo has been initialized in the fixture.
    fn jj_initialized(&self) -> bool;
    /// The resolved root-layout block id post-boot, if present — `StartApp`'s
    /// SUT arg.
    fn root_layout_block_id(&self) -> Option<EntityUri>;
}

/// Reference-side pre-startup boot mutations.
pub trait RefBootMut: RefBoot {
    /// `CreateDirectory`: stage a directory to be created before startup.
    fn push_pre_startup_directory(&mut self, path: &str);
    /// `GitInit`: mark the fixture git-initialized.
    fn mark_git_initialized(&mut self);
    /// `JjGitInit`: mark the fixture jj-initialized (also creates `.git`).
    fn mark_jj_initialized(&mut self);
    /// `StartApp`: the whole boot reference effect — flip `app_started`, seed
    /// the bundled default layout / seed profile / sidebar watch, and
    /// (fresh boot only) open the default drawers + focus `block:journals`.
    fn boot_app(&mut self);
}

/// Reference-side active-watch mutation surface for `SetupWatch` /
/// `RemoveWatch`.
///
/// The watch-spec value (query + language) is an integration-test-only type, so
/// it is abstracted as an associated type — the concrete `WatchSpec` /
/// `TestQuery` never appears in a `holon-pbt-core` signature; the wide
/// `ReferenceState` binds it. Watch READS go through the existing [`RefWatch`]
/// surface (`active_watch_ids`), so no read base is added here.
pub trait RefWatchesMut {
    /// The watch specification value (query + language). `ReferenceState` sets
    /// this to its `pbt::query::WatchSpec`; a watch-less pure slice would bind
    /// its own (or `()`).
    type WatchSpec;
    /// `SetupWatch`: register `spec` under `query_id` in the reference's active
    /// watches (last-writer-wins on a repeated id, mirroring the SUT's
    /// `register_watch`).
    fn insert_watch(&mut self, query_id: &str, spec: Self::WatchSpec);
    /// `RemoveWatch`: drop the active watch `query_id` (no-op if absent).
    fn remove_watch(&mut self, query_id: &str);
}

/// Reference-side expand-toggle read surface (`ExpandToggle`'s generator +
/// precondition). Answers what the SUT would RENDER, so an implementation owes
/// both the gestures it recorded (`ui.tab.expanded_toggles`) and any authored
/// `default_expanded` its render context supplies.
///
/// `#[capmap_adapter]` hosts the read side on `CapMap` (sync, owned return) so
/// composed-slice invariants can gate on it via `Needs`, exactly like sibling
/// read caps (`RefClock`, `RefLayout`). The mutation half stays on
/// [`RefToggleMut`] without the adapter — mutations drive the concrete
/// `ReferenceState` through `apply_to_ref`, not the shared `CapMap`.
#[holon_macros::capmap_adapter]
pub trait RefToggle {
    /// True iff `id`'s `expand_toggle` widget is currently expanded.
    fn is_expanded(&self, id: &EntityUri) -> bool;
}

/// Reference-side journals-feed membership, in the feed's own render order.
///
/// Scopes `inv-journal-feed-viewport-lazy` to the pages the feed draws: an
/// embedded page reached any other way keeps the plain collapsed-lazy rule,
/// whose authority is [`RefToggle::is_expanded`] alone.
#[holon_macros::capmap_adapter]
pub trait RefJournalFeed {
    /// Day pages the journals feed draws, newest first. Empty when the feed is
    /// not the rendered Main root — the feed draws only while `block:journals`
    /// IS that root, so every other topology has no feed pages at all.
    fn feed_day_pages(&self) -> Vec<EntityUri>;
}

/// Reference-side toggle-widget mutations (`ExpandToggle` / `ToggleCollapse` /
/// `ToggleDrawer`). Each flip is single-sourced here so the generic transitions
/// and the concrete `LayoutRef` adapters share one implementation.
pub trait RefToggleMut: RefToggle {
    /// Set `id`'s expand-toggle expanded state (`ExpandToggle` → `true`,
    /// `ToggleCollapse` → `false`), including any document `collapsed` field
    /// mutation the production chevron handler dispatches. Use
    /// [`set_expanded_view_local`] for profile-driven toggles whose collapse is
    /// purely view-local.
    fn set_expanded(&mut self, id: &EntityUri, expanded: bool);
    /// Set `id`'s expand-toggle expanded state WITHOUT modelling any document
    /// `collapsed` field mutation. For profile-driven toggles (e.g.
    /// `embedded_page`) where the toggle's lazy-gate semantics are entirely
    /// view-local — no `collapsed` column exists on the target block. Default
    /// delegates to [`set_expanded`] so existing explicit-render-expr toggle
    /// impls keep the two-field semantics.
    fn set_expanded_view_local(&mut self, id: &EntityUri, expanded: bool) {
        self.set_expanded(id, expanded);
    }
    /// `ToggleDrawer`: flip the drawer panel `id`'s open/closed bit
    /// (default-open, so an untracked drawer flips to closed).
    fn toggle_drawer(&mut self, id: &str);
}

/// Reference-side render-expression read surface (`ExpandToggle`'s candidate
/// enumeration). The render-expr AST (`holon_api::render_types::RenderExpr`)
/// stays inside the impl — callers get ids / bools / a "mentions this builtin"
/// predicate, never the AST itself.
pub trait RefRenderExpr {
    /// Block ids that currently carry a render expression
    /// (`domain.render_expressions` keys).
    fn render_expr_ids(&self) -> Vec<EntityUri>;
    /// True iff `id` has a render expression.
    fn has_render_expr(&self, id: &EntityUri) -> bool;
    /// True iff `id`'s render expression mentions the value-fn builtin `needle`
    /// (e.g. `"expand_toggle"`). False when `id` has no render expression.
    fn render_expr_mentions(&self, id: &EntityUri, needle: &str) -> bool;
}

/// Reference-side view-selection mutation (`SwitchView`). The read side is the
/// existing [`RefViewSelection`] (`current_view`); a standalone mut trait
/// suffices because `SwitchView` writes without reading view state.
pub trait RefViewSelectionMut {
    /// Set the current view filter (`"all"` / `"main"` / `"sidebar"`).
    fn set_current_view(&mut self, view: &str);
}

/// Reference-side wiring/config reads the mutation transitions gate on.
/// Backing: `cap_set` (the composed CapMap discriminator). `enable_loro` stays
/// on [`RefLifecycle`]; this trait carries only what isn't already exposed.
pub trait RefWiring {
    /// True iff this reference carries a composed `cap_set` (i.e. it is a
    /// composed config, not the monolithic `E2ESut`). `SetEdgeField` /
    /// `ApplyMutation` gate their Loro-authority-dependent arms on this.
    fn has_cap_set(&self) -> bool;

    /// True iff every cap in `caps` is present in this reference's composed cap
    /// set. `ApplyMutation` gates its External/seam arm on `SutSeamMutate`
    /// presence (native always; composed only when a frontend seam is wired).
    fn caps_available(&self, caps: &[crate::composition::CapId]) -> bool;
}

/// Reference-side wide-PBT layout / render / focus read surface for the
/// block-interaction transitions (`ClickBlock`, `TriggerSlashCommand`,
/// `DragDropBlock`, `SetEdgeField`, `BulkExternalAdd`). A ref-only surface
/// (never hosted on the CapMap) whose reads only the wide `ReferenceState` can
/// answer — a pure slice has no layout/render model, so it simply doesn't
/// implement it.
pub trait RefLayoutInteract {
    /// Ids of render-source blocks (`layout_blocks.render_source_ids`).
    fn render_source_ids(&self) -> BTreeSet<EntityUri>;
    /// Ids of query-source blocks (`layout_blocks.query_source_ids`).
    fn query_source_ids(&self) -> BTreeSet<EntityUri>;
    /// True iff `id` is an immutable layout block
    /// (`layout_blocks.is_immutable`).
    fn is_immutable(&self, id: &EntityUri) -> bool;
    /// True iff the active layout renders `id` as a `draggable(...)` in the
    /// main panel (shadow-interpreted) — `DragDropBlock`'s source gate.
    fn block_renders_draggable(&self, id: &EntityUri) -> bool;
    /// Block ids in the main panel's active-layout rendered set.
    fn main_rendered_block_ids(&self) -> BTreeSet<EntityUri>;
    /// The click-focused entity in `region` (`focused_entity_id[region]`).
    fn region_focused_entity(&self, region: CapRegion) -> Option<EntityUri>;
    /// The currently-focused editable block in Main (`DragDropBlock`'s source).
    fn focused_main_editable(&self) -> Option<EntityUri>;
    /// True iff `id`'s block carries `tag` in its `tags`.
    fn block_has_tag(&self, id: &EntityUri, tag: &str) -> bool;
    /// True iff `doc_uri` has at least one editable (Text, non-page,
    /// non-layout) child block — `BulkExternalAdd`'s empty-doc weighting.
    fn doc_has_editable_text(&self, doc_uri: &EntityUri) -> bool;

    /// Layout headline block ids (`layout_blocks.headline_ids`) — the candidate
    /// set for `ApplyMutation`'s layout-headline mutation arm.
    fn headline_ids(&self) -> Vec<EntityUri>;
}

/// Reference-side high-level content-mutation apply for `ApplyMutation`'s
/// non-peer arm. The single method performs ALL the internal reference
/// bookkeeping — block-tree apply + canonical re-sequencing, document routing
/// for `Create`, profile-tracking rebuild, render-expr cache refresh, next-id
/// bump, and cursor reset on content updates — so the generic `apply_to_ref`
/// never touches reference internals. Wide-only (only `ReferenceState`
/// implements it); the pure editor slice models no generic mutations. The
/// payload is the pbt-core-native [`crate::types::Mutation`], so no associated
/// type is needed.
pub trait RefApplyMutationMut {
    /// Apply a non-peer (`UI`/`External`) content `mutation` and perform all
    /// the encapsulated reference bookkeeping described on the trait.
    /// Undo-snapshot (UI only) and focus-clear-on-delete stay OUTSIDE this
    /// method — the generic `apply_to_ref` drives them through
    /// [`RefBlockTreeMut::push_undo_snapshot`]
    /// and [`RefFocusMut::clear_focus_if_deleted`].
    ///
    /// `crosses_org_boundary` is true for an `External` (org-file) mutation:
    /// the block is written to disk and RE-INGESTED, so the ref must model
    /// the org FILE-parse reinterpretation (a trailing `:tag:` group on the
    /// headline — e.g. `:PROPERTIES:` — re-parses into `block.tags`, not
    /// content). A `UI` mutation stays in-store (its writeback echo is
    /// suppressed), so the raw content is kept and this is false.
    fn apply_content_mutation(
        &mut self,
        mutation: &crate::types::Mutation,
        crosses_org_boundary: bool,
    );
}

/// Reference-side wide-PBT block-interaction mutation surface. Each method is
/// the whole per-transition reference effect, encapsulated so
/// integration-test-only row/AST internals never cross into `holon-pbt-core`.
/// Wide-only (no pure-slice impl); the payload types (`Region`,
/// `EdgeFieldUpdate`, `Block`) are all `holon_api` and thus pbt-core-nameable.
pub trait RefLayoutMutate {
    /// `ClickBlock`: focus `block_id` via a click in `region` — blur any other
    /// active editor, then either push a navigation-history entry (sidebar
    /// nav-focus) or set editor focus, mirroring `provider.rs`.
    fn apply_click_focus(&mut self, region: holon_api::Region, block_id: &EntityUri);
    /// `TriggerSlashCommand`: snapshot undo, delete `block_id` via the shared
    /// mutation machinery, and clear focus if it pointed at the deleted block.
    fn apply_slash_delete(&mut self, block_id: &EntityUri);
    /// `SetEdgeField`: assign the edge field
    /// (`tags`/`requires`/`advice_suppressed`) carried by `update` on the
    /// existing block `id`.
    fn set_edge_field_value(&mut self, id: &EntityUri, update: &EdgeFieldUpdate);
    /// `BulkExternalAdd`: insert `blocks` under `doc_uri` (org round-trip
    /// normalized), register doc ownership, re-canonicalize, and advance the
    /// block-id counter.
    fn bulk_add_blocks(&mut self, doc_uri: &EntityUri, blocks: &[holon_api::block::Block]);

    /// `CreateBlockUnderFocus`: append a new text block carrying `content` as
    /// the last child of `parent` (the focused page's creation-slot parent).
    /// Mints a synthetic `block::create-N` id the harness reconcile pairs 1:1
    /// with the SUT's minted uuid.
    fn create_block_under(&mut self, parent: &EntityUri, content: &str);

    /// `CreateBlockUnderFocus` with an explicit, born-equal id: append a new
    /// text block carrying `content` as the last child of `parent`, using
    /// exactly `id` (a fresh normal id, NOT a synthetic `create-N`). The
    /// SUT dispatches the same id, so both sides born-equal — no
    /// synthetic→real reconcile.
    fn create_block_under_with_id(&mut self, parent: &EntityUri, content: &str, id: EntityUri);

    /// `CreateBlockUnderFocus{id: None}`: the creation-slot GESTURE, which is
    /// two operations rather than one. Focus reaching the affordance births an
    /// EMPTY block under `parent` as a non-user (undo-invisible) op, and the
    /// text the user types is a separate undo-visible content write — so an
    /// undo after this gesture reverts the text and leaves the empty block.
    /// See `ReferenceState::birth_block_under_slot` for the spec citation.
    fn birth_block_via_creation_slot(&mut self, parent: &EntityUri, content: &str);

    /// `InstantiateTemplate`: seed one template-DEFINITION block, mirroring the
    /// driver's idempotent `block.create`. `parent` is the block's REAL parent
    /// (the definition root for a definition child), so `parent_id` agrees with
    /// the SUT everywhere it is observed — including watch rows, which apply no
    /// seed filter. The block is nevertheless classified as SEED (its
    /// `block_documents` entry is forced to `no_parent`), which is what keeps
    /// template-engine internals out of the block-comparison invariants. Parent
    /// truthfulness and seed classification are independent axes; conflating
    /// them by parenting definitions at `no_parent` made every definition child
    /// report the document-root sentinel to `inv-watch-rows-match-ref`.
    fn seed_template_definition(
        &mut self,
        parent: &EntityUri,
        content: &str,
        marks: Option<Vec<holon_api::MarkSpan>>,
        id: EntityUri,
    );

    /// `InstantiateTemplate`: mint the instance subtree (root + child) under
    /// `target_parent` as ONE undoable unit, so a single `UndoLastMutation`
    /// removes the whole instantiation. The ids are caller-computed and must
    /// be born-equal with the SUT's; the instance root carries `instance_of`.
    /// `child_marks` are the definition child's marks after `{{var}}`
    /// substitution has shifted them — an instance inherits the definition's
    /// rich text, it is not born plain.
    fn apply_instantiate_template(
        &mut self,
        target_parent: &EntityUri,
        inst_root_id: EntityUri,
        inst_child_id: EntityUri,
        root_content: &str,
        child_content: &str,
        child_marks: Option<Vec<holon_api::MarkSpan>>,
        template_id: &str,
    );

    /// `BlockToPage` (Option B): mint a NEW page block `page_id` (Page-tagged,
    /// its own document) as the last child of `destination_parent`, re-home
    /// `origin`'s direct children (and their non-page subtrees) under it, and
    /// leave `origin` in place as a non-page carrying a full-span `[[page_id]]`
    /// Link mark. Mirrors the engine-level `convert_block_to_page` compound so
    /// the born-equal page id / reparented children / origin marks all match
    /// the SUT for the block-comparison invariants.
    fn apply_block_to_page(
        &mut self,
        origin: &EntityUri,
        page_id: EntityUri,
        destination_parent: &EntityUri,
    );
}

/// Reference-side arrow-key navigation surface (`ArrowNavigate`). The
/// direction type is **associated** so `holon-pbt-core` needn't depend on
/// `holon-frontend` (mirrors [`RefWatchesMut::WatchSpec`]); the
/// integration-test `ReferenceState` binds `Direction =
/// holon_frontend::navigation::NavDirection`. The whole cross-block
/// cursor/focus walk is encapsulated because it drives `holon_frontend`'s
/// `CollectionNavigator`, which is not nameable here.
pub trait RefArrowNav {
    /// Arrow-key direction (`holon_frontend::navigation::NavDirection` in the
    /// wide PBT).
    type Direction;

    /// Whether `region` currently has a focused entity — the arrow-nav
    /// precondition. Region-granular (Left/RightSidebar distinct), so it does
    /// not collapse through `CapRegion`.
    fn region_has_focus(&self, region: holon_api::Region) -> bool;

    /// Apply `steps` arrow presses in `direction` from the focused block of
    /// `region`: moves editor focus + cursor (navigation history untouched),
    /// mirroring production's GPUI arrow handler.
    fn apply_arrow_navigate(
        &mut self,
        region: holon_api::Region,
        direction: Self::Direction,
        steps: u8,
    );
}

/// Reference-side task-state toggling surface (`ToggleState`). The candidate
/// computation interprets the render expr (via
/// `holon_frontend::interpret_pure`), so its body lives in the integration-test
/// `ReferenceState` impl; pbt-core declares only the surface over
/// pbt-core-native [`CycleTarget`].
pub trait RefTaskStateToggle {
    /// Block ids that render an interactive `state_toggle` widget in Main —
    /// the `ToggleState` generator's candidate set before target pairing.
    fn rendered_state_toggle_ids(&self) -> Vec<EntityUri>;

    /// Apply a task-state toggle to the reference model: push an undo snapshot,
    /// then an `Update { task_state }` mutation for `block_id`.
    fn apply_toggle_state(&mut self, block_id: &EntityUri, new_state: CycleTarget);
}

/// A single watch-result row, field name → stringified value. `None`
/// means the column was SQL-NULL or absent (mirrors the inline check's
/// `Value::as_string()` returning `None`). Both sides of
/// `inv-watch-rows-match-ref` carry rows in this normalized shape so the
/// body compares `Option<String>` to `Option<String>` directly, exactly
/// as the inline check did with `.and_then(|v| v.as_string())`.
pub type WatchRow = std::collections::HashMap<String, Option<String>>;

// `capability_pair!` single-sources the watch read duality: the SUT-side
// CDC-driven `ui_model` surface ([`SutWatch`], async) and the reference
// active-watches model ([`RefWatch`], sync, owned). The `#[compare]` method
// auto-derives `inv-active-watches-match-ref` (id preserved — asserted by id
// in slice teeth) via [`compare_watch_ids`]; the watch *rows* comparison
// (`inv-watch-rows-match-ref`) stays a hand-written invariant (per-watch
// loop + per-field CDC-lag classifier, not a two-value compare), but its cap
// methods live here as `#[sut_only]` / `#[ref_only]`.
holon_macros::capability_pair! {
    /// Watch read surface: SUT CDC-delivered `ui_model` watch state (separate
    /// from [`SutSqlProjection`] so the per-id String surface there stays
    /// focused) vs the reference's registered active watches. Registered only
    /// where watches are actually driven; a watch-less slice does NOT register
    /// it, so the watch invariants honestly DESELECT there.
    pub trait Watch {
        /// Query ids of currently registered watches. SUT: keys of
        /// `TestContext::ui_model`; reference: keys of
        /// `ReferenceState::active_watches` (pure slice: empty). Auto-compared
        /// (set semantics) by `inv-active-watches-match-ref` via
        /// [`compare_watch_ids`].
        #[compare(
            ref = active_watch_ids,
            id = "inv-active-watches-match-ref",
            layer = ViewModel,
            with = crate::capabilities::compare_watch_ids
        )]
        fn watch_query_ids(&self) -> Vec<String>;

        /// CDC-delivered rows for the watch `query_id`, stringified into the
        /// [`WatchRow`] shape. Wide PBT: `ui_model[query_id].to_vec()` with each
        /// `Value` mapped through `as_string()`. Empty if `query_id` is not
        /// registered.
        #[sut_only]
        fn watch_rows(&self, query_id: &str) -> Vec<WatchRow>;

        /// Run the given SQL against the SUT's DB and return the set of `id`
        /// values it yields (e.g. `SELECT id FROM block_raw`). Wide PBT:
        /// `ctx.query_sql(sql)` projecting the `id` column.
        #[sut_only]
        fn block_raw_query_ids(&self, sql: &str) -> BTreeSet<EntityUri>;

        /// Expected result rows for the watch `query_id`, stringified into the
        /// [`WatchRow`] shape. Wide PBT: `query_results(active_watches[query_id])`
        /// evaluated against the (already SUT-ID-space-resolved) block state;
        /// pure slice: empty. Returns an empty Vec if `query_id` is not a
        /// registered watch.
        #[ref_only]
        fn expected_watch_rows(&self, query_id: &str) -> Vec<WatchRow>;

        /// The selected columns of the watch `query_id` — the field set the
        /// per-row comparison checks. Wide PBT: `active_watches[query_id].query.columns`;
        /// empty if `query_id` is unknown.
        #[ref_only]
        fn watch_query_columns(&self, query_id: &str) -> Vec<String>;
    }
}

/// Comparator for `inv-active-watches-match-ref` (the `Watch` pair's
/// `#[compare]`): the registered watch id sets agree, order-insensitively.
/// Lifted verbatim from the deleted hand-written body — the watch *rows* are
/// checked separately by `inv-watch-rows-match-ref`; this is just the
/// subscription-set agreement.
pub fn compare_watch_ids(sut_ids: &[String], ref_ids: &[String]) -> Result<(), String> {
    let sut: BTreeSet<&String> = sut_ids.iter().collect();
    let ref_: BTreeSet<&String> = ref_ids.iter().collect();
    if sut == ref_ {
        return Ok(());
    }
    let missing: Vec<&&String> = ref_.difference(&sut).collect();
    let spurious: Vec<&&String> = sut.difference(&ref_).collect();
    Err(format!(
        "[inv-active-watches-match-ref] watch sets diverged\n  missing on SUT: {missing:?}\n  \
         spurious on SUT: {spurious:?}"
    ))
}

/// Global engine-focused block (distinct from the per-region navigation
/// focus). Set by click handlers in the reactive engine; read by
/// `inv-focus-matches-ref` to compare against `ReactiveEngine::focused_block`.
///
/// `#[capmap_adapter]` hosts this on `CapMap` (sync, owned return). The
/// production `ReferenceState` provides it; the composed ref `CapMap` forwards.
#[holon_macros::capmap_adapter]
pub trait RefGlobalFocus {
    /// The globally focused block id, or `None` if nothing is focused.
    /// Wide PBT: `ReferenceState::focused_block`; pure slice: `None`.
    fn global_focused_block(&self) -> Option<EntityUri>;
}

/// Task-state read-side projection. Used by
/// `inv-viewmodel-state-toggle-correct` to compare block task_state values
/// against ViewModel StateToggle nodes.
///
/// `#[capmap_adapter]` hosts this on `CapMap` (sync, owned return). The
/// production `ReferenceState` provides it; the composed ref `CapMap` forwards.
#[holon_macros::capmap_adapter]
pub trait RefTaskState {
    /// Task state string for `id` (`"TODO"`, `"DONE"`, etc.), or `None`
    /// if the block has no task_state property.
    fn task_state_of(&self, id: &EntityUri) -> Option<String>;
}

/// SQL-budget cardinality inputs: the handful of reference-state counts the
/// per-transition `SqlBudget` formulas read. Lets a transition's
/// `SqlBudget::expected_sql` be generic over `R` instead of binding the
/// concrete `ReferenceState` (Phase 1a Step 1). `last_navigate_first_visit`
/// rides along because `NavigateFocus`'s budget switches on it (first visit
/// creates watch matviews); it is a per-step budget input, not a nav mutation.
pub trait RefSqlCardinality {
    fn block_count(&self) -> usize;
    fn document_count(&self) -> usize;
    fn active_watch_count(&self) -> usize;
    fn last_navigate_first_visit(&self) -> bool;
    /// True iff the last `open_tab` found its `(region, block_id)` row already
    /// open and delegated to `activate`. `OpenTabViaModifierClick`'s budget
    /// switches on it because the two branches issue different SQL, and the
    /// post-apply state cannot tell them apart.
    fn last_open_tab_activated(&self) -> bool;

    /// True iff a block-content write reaches SQL as a write — i.e. SQL, not
    /// Loro, holds block CRUD. Per-keystroke write budgets fork on it: in the
    /// Loro arm the content lands in the CRDT and only the undo journal is a
    /// SQL write, in the SqlOnly arm both are.
    fn content_writes_reach_sql(&self) -> bool;

    /// How many block merges the most recent `DeleteBackward` performed.
    /// A backspace at caret 0 is a JOIN, not a keystroke, so its budget is
    /// `JoinBlock`'s — see
    /// [`RefEditorMirrorMut::note_backspace_joins`] for why it is recorded
    /// rather than derived.
    fn last_backspace_joins(&self) -> usize;
}

/// Reference-side typed block surface for `inv-backend-blocks-match-ref`.
///
/// The runner already remaps the reference model into SUT ID space
/// (`with_resolved_doc_uris`), so the blocks this returns carry resolved
/// `id`/`parent_id` and can be compared directly against
/// [`SutBackend::live_block_snapshot`]. Coupled to `holon_api::Block` for
/// the same reason as [`SutBackend`]: the deep field-level comparison
/// needs typed values, not the format-agnostic id/content surface of
/// [`RefBlockTree`].
#[holon_macros::capmap_adapter] // sync trait → no async-trait; emits CapName + `impl … for CapMap`
pub trait RefBackend {
    /// All reference blocks EXCLUDING seed blocks (those whose document is
    /// `no_parent`/`sentinel` — inserted via direct SQL, never reverse-synced
    /// to the matview the backend comparison reads). Mirrors the monolith's
    /// `ref_blocks_no_seed`.
    fn non_seed_blocks(&self) -> Vec<holon_api::Block>;

    /// Resolved (SUT-ID-space) seed block ids. Used to filter seed rows out
    /// of the SUT's `block_raw` id set during the CDC-lag truth check.
    /// Mirrors the monolith's translated `seed_block_ids`.
    fn seed_block_ids(&self) -> BTreeSet<EntityUri>;

    /// Reference blocks as they should appear ON DISK in org files:
    /// non-seed, non-page (org files hold no page blocks), with document
    /// parents resolved into the org parser's id space — `block:<uuid>` for
    /// `#+ID:`-resolved docs (already remapped by `with_resolved_doc_uris`),
    /// `file:<filename>` for docs the controller hasn't resolved yet. Mirrors
    /// the monolith's `ref_blocks_org_only`; compared against
    /// [`SutOrgRead::org_block_snapshot`] by `inv-blocks-match-ref/org`.
    fn org_blocks(&self) -> Vec<holon_api::Block>;
}

// ─── Cross-cut helpers ───────────────────────────────────────────────

/// Cross-cut helper used by `TypeChars::apply_to_ref` and
/// `DeleteBackward::apply_to_ref` when Loro is enabled. Reads the active
/// editor's pending text, commits it to `block_content` of the focused
/// block. Lifted from `ReferenceState::commit_active_editor_if_changed`.
///
/// Returns `true` if a commit happened; `false` if no editor was active
/// or content already matched.
pub fn commit_active_editor_if_changed<R>(state: &mut R) -> bool
where
    R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus,
{
    let (block_id, text) = match (
        state.active_editor_block(),
        state.active_editor_text().map(|s| s.to_owned()),
    ) {
        (Some(id), Some(t)) => (id, t),
        _ => return false,
    };
    // The editor buffer is the block's SOURCE PROJECTION, so "unchanged" is
    // measured against that surface, not against the content column it is
    // derived from.
    let surface = state.editor_surface_text(&block_id);
    if surface == text {
        state.mark_active_editor_committed();
        return false;
    }
    if holon_org_format::source_channel_commit(&surface, &text) {
        state.commit_editor_source(&block_id, &text);
    } else {
        state.set_block_content(&block_id, &text);
    }
    state.mark_active_editor_committed();
    // The store canonicalizes what it stored (`TODO  milk` is the task `milk`),
    // and the editor converges onto that canonical surface — except for the one
    // shape its echo discriminator recognises as its own (trailing whitespace).
    // The reference applies the SAME discriminator.
    state.refresh_clean_editor_surface(&block_id);
    true
}

/// Commit the active editor's pending text only if it is DIRTY — i.e. the
/// text was authored by modeled typing/deleting, not merely divergent from
/// `block.content` via an external change (a stale mirror; prod's data
/// subscription would have refreshed it, so committing it would write old
/// text into the ref). This models prod's blur / structural-commit-point
/// behavior: "structural ops are commit points" (docs/Architecture/UI.md),
/// and a click-away blur commits the previously focused editor's
/// user-authored text.
pub fn commit_active_editor_if_dirty<R>(state: &mut R) -> bool
where
    R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus,
{
    if !state.active_editor_dirty() {
        return false;
    }
    commit_active_editor_if_changed(state)
}

// ── Formerly integration-test-local SUT caps (Phase 1a Step 2 / B1) ─────────
// Relocated from `holon-integration-tests::pbt::local_caps` (and the
// apply_mutation / start_app transition modules) once the types they name
// (`CycleTarget`, `MutationEvent`, `LoroCorruptionType`) moved to
// `crate::types`. This unblocks the mutation/fixture-driven transitions +
// SUT adapters co-locating into companion `*-testing` crates.
use crate::types::CycleTarget;
use crate::types::LoroCorruptionType;
use crate::types::MutationEvent;

/// SUT capability: task-state cycling (`ToggleState`). A genuinely composable
/// `&self` mutation realized headlessly via the production `set_field
/// task_state` op.
#[holon_macros::capmap_adapter]
pub trait SutMutate {
    async fn toggle_state(&self, block_id: &holon_api::EntityUri, new_state: CycleTarget);
}

/// SUT capability: the SEAM-relocated mutations — generic UI/external mutations
/// (`ApplyMutation`) and bulk external block adds (`BulkExternalAdd`). Their
/// real, `ref_state`-dependent dispatch lives in the `E2ESut` harness seam;
/// the composed frontend does NOT provide it, so those transitions auto-narrow
/// out of the composed alphabet rather than faking a no-op.
#[holon_macros::capmap_adapter]
pub trait SutSeamMutate {
    async fn apply_mutation(&self, event: MutationEvent);
    async fn bulk_external_add(
        &self,
        doc_uri: &holon_api::EntityUri,
        blocks: &[holon_api::block::Block],
    );
    /// Replay a STALE external snapshot over the doc's file: re-render the
    /// doc's CURRENT content but with every block `:ID:` drawer stripped, as
    /// an editor/agent writing from a pre-writeback copy (before Holon minted
    /// the ids) would. Re-ingest must reconcile the id-less blocks against the
    /// store's current children -- NOT duplicate them (the PR #81 bug class).
    async fn stale_external_rewrite(&self, doc_uri: &holon_api::EntityUri);
}

/// SUT capability: create a block through the focused panel's creation slot
/// (`CreateBlockUnderFocus`). The composed `HeadlessFrontendComponent` realizes
/// it through the PRODUCTION creation-slot commit seam
/// (`ReactiveEngineDriver::commit_creation_slot` →
/// `ViewEventHandler::handle_text_sync` → `block.create`), so the headless
/// keystone drives WP-E's focus-root parenting exactly as a real user's "type
/// here to create" gesture does — the parent comes from the live
/// `:__virtual:<parent>` slot id, never re-derived. A genuinely composable
/// `&self` gesture: any composed config whose frontend renders the default
/// `creation_slot: true` layout can drive it, so the transition auto-narrows to
/// exactly those configs.
#[holon_macros::capmap_adapter]
pub trait SutBlockCreate {
    /// Create a block under `parent` with `content`. When `id` is `Some`, the
    /// backend uses that exact (born-equal) id; when `None`, the backend MINTS
    /// a fresh id (the `block.create` mint-when-absent path). The `parent`
    /// is the ref-resolved creation-slot focus root; the headless UI driver
    /// honors the production slot gesture for the `None` case (resolving
    /// the parent from its own live rowset), while the op-floor driver
    /// dispatches `block.create` directly with the passed parent.
    async fn apply_create_under_focus(
        &self,
        parent: &EntityUri,
        content: &str,
        id: Option<&EntityUri>,
    );
}

/// SUT capability: advance the injected fake wall clock and let the production
/// `ClockScheduler` propagate the new day through its own reconcile path — the
/// keystone `AdvanceDay` transition's driver (ADR 0024 §6). NEVER a raw
/// `clock`-relation UPDATE behind the scheduler's back (WP1's clock-injection
/// race): the impl advances a `TestClock` the scheduler already reads via DI,
/// then invokes the scheduler's own `reconcile_clock`, so the day-rollover CDC
/// re-fires the journal rule exactly as a real day rollover would. Hosted only
/// where a controllable clock was injected (the frontend arm), so `AdvanceDay`
/// auto-narrows to those configs. Returns the new local `YYYY-MM-DD` date.
#[holon_macros::capmap_adapter]
pub trait SutClockAdvance {
    async fn advance_clock_days(&self, days: i64) -> String;
}

/// SUT capability: read the C2 op/effect history relation (`block_history`)
/// for the keystone provenance correspondences. Hosted only where a real
/// `TursoHistoryStore` is wired (Turso present); an org-only draw omits it, so
/// the history invariants deselect cleanly — no phantom/missed-history claim
/// without a recording substrate.
#[holon_macros::capmap_adapter]
pub trait SutHistory {
    /// The distinct `block_id`s that appear in `block_history` op-group rows —
    /// every block the recorded op/effect stream has touched so far.
    async fn history_block_ids(&self) -> BTreeSet<EntityUri>;
    /// The number of distinct `op_group`s recorded so far (one per
    /// history-recording `execute_operation` call with ≥1 field delta).
    async fn history_op_group_count(&self) -> usize;
}

/// Reference-side expectation for the C2 history relation, derived by the
/// harness from the per-tick id-reconcile map (the authoritative record of
/// which blocks the oracle drove into existence). Feeds the two keystone
/// provenance correspondences: `ever_created_ids` bounds PHANTOM history
/// (every recorded id must be one the oracle knows/knew),
/// `min_recorded_op_groups` bounds MISSED history (each oracle-driven create
/// records ≥1 op_group).
#[holon_macros::capmap_adapter]
pub trait RefHistoryExpectation {
    /// Every real block id the oracle minted via the reconcile — a SUPERSET
    /// anchor for the phantom-history subset check (bigger only weakens the
    /// check, never false-fails it).
    fn ever_created_ids(&self) -> BTreeSet<EntityUri>;
    /// A conservative lower bound on the distinct `op_group`s the SUT must have
    /// recorded: the count of UI-driven (synthetic→real reconciled) creates,
    /// each routed through `execute_operation` recording ≥1 delta. Excludes
    /// born-equal external/peer creates (org-ingest / Loro path — no engine
    /// history), so the bound never over-counts.
    fn min_recorded_op_groups(&self) -> usize;
}

/// Reference-side record of block ids the SUT BURNED across a COMPLETED
/// undo→redo round trip, derived by the harness from the id-reconcile map.
///
/// A pair enters this set only when both halves hold: the SUT no longer has the
/// real id, AND the oracle holds its synthetic label again. That conjunction is
/// exactly "the block came back under a new identity" — it cannot be satisfied
/// by a bare undo (there the oracle has dropped the label too), so a reference
/// left dangling by an undo alone is never reported here. Feeds
/// `inv-undo-redo-reference-heal`.
#[holon_macros::capmap_adapter]
pub trait RefUndoRedoBurned {
    /// The real block ids retired by the reconcile because a redo re-minted
    /// their block under a fresh id. Empty on any run without a completed
    /// undo→redo round trip over a minted block.
    fn burned_block_ids(&self) -> BTreeSet<EntityUri>;
}

/// SUT capability: app lifecycle for the wide PBT — boot, restart, document
/// creation, and the concurrent-schema-init regression probe. `&self`,
/// `ref_state`-free; `ref_state`-derived values are precomputed at the
/// transition boundary and passed as typed args.
#[holon_macros::capmap_adapter]
pub trait SutAppLifecycle {
    #[allow(clippy::too_many_arguments)]
    async fn start_app(
        &self,
        root_id: holon_api::EntityUri,
        expects_valid_index: bool,
        wait_for_ready: bool,
        enable_fake_mcp: bool,
        enable_loro: bool,
    );
    async fn simulate_restart(&self);
    async fn create_document(&self, file_name: &str);
    async fn delete_document(&self, file_name: &str);
    /// `RenameDocument`: move the org file `old_file_name` -> `new_file_name`
    /// on disk (a user-side `mv` in the vault) via `FileSystem::rename`, which
    /// emits ONE atomic `Rename { from }` change. Production's
    /// `FileSyncController::on_file_renamed` re-homes the SAME existing
    /// document (identity kept via its `#+ID:`) with NO delete window and
    /// retitles the page to the new stem — the file-move spec.
    async fn rename_document(&self, old_file_name: &str, new_file_name: &str);
    async fn concurrent_schema_init(&self);
    async fn assert_epoch_flip_rejected(&self);
}

/// SUT capability: pre-startup org-filesystem fixture setup — writing org
/// files, creating directories, `git`/`jj` init, and planting a stale/corrupt
/// Loro snapshot. `E2ESut`-only (no headless filesystem).
#[holon_macros::capmap_adapter]
pub trait SutFixtureFs {
    async fn write_org_file(&self, filename: &str, content: &str);
    async fn create_directory(&self, path: &str);
    async fn git_init(&self);
    async fn jj_git_init(&self);
    async fn create_stale_loro(&self, org_filename: &str, corruption_type: LoroCorruptionType);
}

/// SUT capability: instantiate a block template through the production
/// `block.instantiate_template` operation. The template is canned inline
/// (a `{{date}}` root with one `{{mood}}` child carrying a `Link` mark).
/// Dispatched through the op-floor `DirectUserDriver` which first seeds
/// the template blocks (idempotent `block.create`) then calls
/// `instantiate_template` via the production engine.
#[holon_macros::capmap_adapter]
pub trait SutTemplateInstantiate {
    async fn instantiate_template(
        &self,
        template_id: &EntityUri,
        target_parent: &EntityUri,
        context_key: &str,
        bindings: &[(String, String)],
    );
}

/// SUT capability: register a NEW entity type at runtime through the
/// agent-facing MCP `create_entity_type` tool — the rung an MCP sidecar
/// provider actually connects on. Drives the `RegisterEntityScheme` transition.
///
/// The tool, not `TypeRegistry::register`: the tool also creates the extension
/// table and registers the GQL node, so poking the registry directly would be a
/// lower driver rung than production ever takes and would bake the difference
/// into the harness. Only compositions that can serve MCP tools over the SAME
/// container the link classifier reads insert this cap, so every other slice
/// deselects the transition by cap narrowing.
#[holon_macros::capmap_adapter]
pub trait SutEntityTypeRegister {
    /// Register `entity_name` (the SQL table spelling, e.g. `t_widget`). A tool
    /// error is a loud panic: the reference has already recorded the
    /// registration, so a swallowed failure would resurface as an unrelated
    /// divergence.
    async fn register_entity_type(&self, entity_name: &str);
}

/// Reference-side view of which entity schemes have been registered at runtime.
/// Read ONLY to keep `RegisterEntityScheme` from minting the same entity twice;
/// deliberately NOT read by any link oracle — `block_links` is registration
/// independent, so an expectation that changed with this set would contradict
/// the property.
pub trait RefEntitySchemes {
    fn entity_scheme_registered(&self, entity_name: &str) -> bool;
}

/// Write side of [`RefEntitySchemes`]. Plain trait (no `capmap_adapter`): a
/// `&mut self` cap cannot live behind the `Arc` a `CapMap` stores.
pub trait RefEntitySchemesMut: RefEntitySchemes {
    fn note_entity_scheme_registered(&mut self, entity_name: &str);
}

/// SUT capability: turn an existing non-page block into a page via the
/// production `block.convert_block_to_page` compound (BlockToPageTransform,
/// Option B). Mints a NEW page under the destination, re-homes the origin's
/// children under it, and leaves the origin in place as a `[[page]]` link
/// (the origin itself stays a NON-page). `target` is the origin block id;
/// an EMPTY `destination_path` means "use the backend default" (the origin's
/// nearest page ancestor — the op's `destination_path`-absent branch), so the
/// new page lands under a page and `inv-no-page-under-non-page` stays
/// satisfied. Dispatched through the op-floor `DirectUserDriver`.
#[holon_macros::capmap_adapter]
pub trait SutBlockToPage {
    async fn convert_block_to_page(&self, target: &EntityUri, destination_path: &str);
}

/// SUT capability: the two halves of the **page-identity lifecycle** — retitle
/// an existing page, and lazily create a page from a wiki-link target.
///
/// Both are production op dispatches through the op-floor `DirectUserDriver`:
///
/// * `rename_page` → `block.set_field("content")` on the page block. Per
///   `docs/Plans/PageIdentityDeterminism.md` §5.3 a rename is "an ordinary edit
///   to the existing entity" — the id does NOT re-mint.
/// * `create_page_from_link` → `block.create_page_from_link(target)`, the path
///   a click on a newly typed `[[Target]]` takes when the page doesn't exist
///   yet. It mints each missing segment's id as
///   `PageId::for_path(accumulated_path)`.
///
/// They ship as ONE cap because the property that needs them is temporal and
/// needs BOTH: a page name has to be *freed* by a rename before a later
/// creation can re-mint it. A config that hosts only one half could never reach
/// the state the `inv-blocks-match-ref` oracle judges, so splitting the cap
/// would only buy a vacuous narrowing.
#[holon_macros::capmap_adapter]
pub trait SutPageIdentity {
    /// Retitle the existing page `page` to `new_title` (ordinary content edit,
    /// same entity, same id).
    async fn rename_page(&self, page: &EntityUri, new_title: &str);
    /// Lazily create the page chain named by the `/`-joined `target`.
    async fn create_page_from_link(&self, target: &str);
}

/// Reference-side page-identity surface (`RenamePage` /
/// `CreatePageAtFreedPath`).
///
/// Wide-only, like [`RefLayoutMutate`]: pages are a wide-PBT concept and the
/// pure slice has none. The freed-path ledger is the **temporal** half of the
/// generator — a page name can only be re-created after a rename has freed it,
/// and nothing else in the reference records that a name once belonged to a
/// page that no longer carries it.
pub trait RefPageIdentity {
    /// Page paths (`/`-joined, root→leaf) that a `RenamePage` has vacated and
    /// that no page currently occupies. The `CreatePageAtFreedPath` generator
    /// draws from this pool; empty until a rename happens.
    fn freed_page_paths(&self) -> Vec<String>;

    /// The `/`-joined page path (root→leaf) of an existing page block, or
    /// `None` when `id` is not a page or any page in its chain has empty
    /// content. Mirrors the backend planner's `page_path_of`.
    fn page_path_of_ref(&self, id: &EntityUri) -> Option<String>;

    /// Reference mirror of the backend's `resolve_page_name` — the title-only
    /// lookup `create_page_from_link` uses per segment. Needed by the
    /// `CreatePageAtFreedPath` generator so it only offers a path whose
    /// ancestor chain already resolves and whose LEAF does not, i.e. a path the
    /// op must mint exactly one page for.
    fn ref_resolve_page_name(&self, hint: &str) -> Option<EntityUri>;

    /// Trimmed titles of EVERY page in the reference, seed pages included. The
    /// `RenamePage` generator refuses a new title already in use: the backend's
    /// `resolve_page_name` matches on title alone, so a duplicate title makes
    /// which page a later link resolves to depend on id order rather than on
    /// the model.
    fn page_titles(&self) -> Vec<String>;

    /// `RenamePage`: retitle the existing page `page_id` to `new_title`. The id
    /// is untouched (§5.3) and the page's OLD path enters the freed-path pool.
    fn apply_page_rename(&mut self, page_id: &EntityUri, new_title: &str);

    /// `CreatePageAtFreedPath`: mirror `create_page_from_link(path)` — walk the
    /// `/`-joined `path` segment by segment, reusing a page whose title already
    /// resolves and minting the missing ones.
    ///
    /// §5.3: "A *new* page created later under the new name gets a new id; that
    /// is correct (it is a different logical page)." So a minted segment whose
    /// deterministic `PageId::for_path` id is ALREADY occupied — the exact
    /// state a rename leaves behind — must still become a **distinct entity**,
    /// never an overwrite of the occupant.
    fn apply_create_page_at_path(&mut self, path: &str);
}

// ─── Two-instance true sharing (SyncTransport plan Inc0/Inc1) ─────────

/// What ONE bounded sync round did, as the PBT observes it. The counters are
/// the executed-witness: a "nothing was transported" assertion is only
/// meaningful beside proof that the round RAN and consulted the transport.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncRoundWitness {
    /// Rounds driven so far on this SUT (monotonic).
    pub rounds_run: u64,
    /// Containers the last round walked. `0` means the orchestrator did
    /// nothing.
    pub containers_visited: usize,
    pub pushed: usize,
    pub imported: usize,
    /// Rendered acceptor refusals — typed decisions, never silent drops.
    pub refusals: Vec<String>,
    /// Logs on the transport this side has no container mounted for.
    pub unmounted: Vec<String>,
    /// Containers the publisher declined to publish for want of a membership
    /// proof — the "nothing left the device" witness on an unshared vault.
    pub unauthorized: Vec<String>,
    /// Cumulative transport consultations (push + pull + list) across all
    /// rounds, read off the relay itself.
    pub transport_consultations: u64,
    /// Total envelopes the transport holds — the "was anything transported at
    /// all" observable.
    pub transport_envelopes: usize,
}

/// SUT-side control of a TWO-INSTANCE (owner + receiver) composed slice: share
/// a container and drive one bounded sync round. Absent on every
/// single-instance slice, so the sharing transitions and invariants deselect
/// there by cap selection — disclosed, not faked.
#[holon_macros::capmap_adapter]
pub trait SutTwoInstance {
    /// `(owner, receiver)` Loro peer ids the slice INTENDED to inject. Distinct
    /// by construction — the whole point of the peer-id injection seam.
    async fn instance_peer_ids(&self) -> (u64, u64);

    /// `(owner, receiver)` peer ids the LIVE documents actually carry. Separate
    /// from [`Self::instance_peer_ids`] so a test can catch an injection seam
    /// that reports an intention it never applied.
    async fn live_doc_peer_ids(&self) -> (u64, u64);

    /// Grant `principal` membership over `selector` and put the container into
    /// the replication set. Idempotent.
    async fn share_container(&self, selector: &str, principal: &str);

    /// One bounded sync round. `owner_to_receiver` picks which side publishes.
    async fn sync_now(&self, owner_to_receiver: bool) -> SyncRoundWitness;

    /// The round witness WITHOUT driving a round — for an invariant that must
    /// read counters it did not produce.
    async fn sync_witness(&self) -> SyncRoundWitness;
}

/// SUT-side observation of the RECEIVER instance's projections. Separate from
/// [`SutBackend`] (which is always the owner) so an invariant naming this cap
/// cannot accidentally read the owner and pass vacuously.
#[holon_macros::capmap_adapter]
pub trait SutReceiverBackend {
    /// Block ids in the receiver's store, after settle.
    async fn receiver_block_ids(&self) -> BTreeSet<EntityUri>;

    /// Block ids the receiver's ORG files carry — the receiver-side writeback
    /// round-trip half of convergence.
    async fn receiver_org_block_ids(&self) -> BTreeSet<EntityUri>;

    /// Block ids in the OWNER's store RIGHT NOW.
    async fn owner_block_ids(&self) -> BTreeSet<EntityUri>;

    /// Block ids the owner held when the last owner→receiver round ran — i.e.
    /// exactly what that round could have carried. Empty before any round.
    ///
    /// Convergence must be judged against THIS, not against the owner's live
    /// set: a block created after the last round has not been offered to the
    /// receiver yet, so demanding it would fail a correct system. The set is
    /// still fully load-bearing — anything that existed at push time and did
    /// not arrive is a real loss.
    async fn owner_block_ids_at_last_round(&self) -> BTreeSet<EntityUri>;

    /// Ids the receiver already held at boot (its own disjoint seed + the
    /// programmatic default layout, which both instances mint under the same
    /// fixed ids). `owner_block_ids − receiver_boot_block_ids` is the
    /// OWNER-EXCLUSIVE set: the only ids whose presence on the receiver proves
    /// transport actually happened.
    async fn receiver_boot_block_ids(&self) -> BTreeSet<EntityUri>;

    /// The owner's ORG-carried block ids — the identity landing-zone reference
    /// for the receiver's org half (whole-vault self-device sync maps paths
    /// 1:1).
    async fn owner_org_block_ids(&self) -> BTreeSet<EntityUri>;

    /// CRDT-level convergence: do the two instances' documents reach the same
    /// state under a pairwise fork-and-sync fixed point? `None` when either
    /// document is unavailable (the invariant then skips that layer, honestly).
    async fn crdt_converged(&self) -> Option<bool>;
}

/// Reference-side view of what the receiver is ENTITLED to hold, and how many
/// sync rounds the model believes have run. Ref-only; the SUT twin is
/// [`SutReceiverBackend`].
#[holon_macros::capmap_adapter]
pub trait RefSharedView {
    /// Is the vault (root container) shared with the receiver right now?
    fn is_shared(&self) -> bool;

    /// The principal the receiver acts as.
    fn receiver_principal(&self) -> String;

    /// Owner→receiver sync rounds the model has applied. `0` = nothing may have
    /// crossed yet, whatever the SUT did.
    fn owner_to_receiver_rounds(&self) -> u64;

    /// The audience the container is shared with. Empty ⇒ local-only.
    fn shared_audience(&self) -> Audience;
}

/// Write side of [`RefSharedView`] — the two model mutations the sharing
/// transitions perform. Plain trait (no `capmap_adapter`): a `&mut self` cap
/// cannot live behind the `Arc` a `CapMap` stores, exactly like
/// [`RefBlockTreeMut`].
pub trait RefSharedViewMut: RefSharedView {
    /// Widen the WHOLE-VAULT policy audience to include `principal` and bump
    /// the sharing epoch. Policy widens FIRST (H3): after this, every block —
    /// including ones created later — is policy-covered, so the effective
    /// audience can never over-approximate it.
    fn apply_share_vault(&mut self, principal: &str);

    /// Record one applied owner→receiver sync round.
    fn note_owner_to_receiver_round(&mut self);
}

#[cfg(test)]
mod attribution_tests {
    /// `capability_pair!` emits `file!()` INTO the generated constructor, so a
    /// derived invariant's wiring names the file that declared the pair rather
    /// than `holon-macros`. Without it all three pair-derived invariants would
    /// point triage at the macro crate.
    #[test]
    fn derived_pair_invariants_are_wired_to_their_declaring_file() {
        for inv in [
            super::inv_pair_focus_current_focus_rows(),
            super::inv_pair_watch_watch_query_ids(),
            super::inv_pair_view_selection_current_view(),
        ] {
            let wiring = inv.attribution().wiring();
            assert!(
                wiring.ends_with("capabilities.rs"),
                "{} wired to {wiring}, expected the declaring file",
                inv.id().0,
            );
        }
    }
}
