//! Metadata-only invariant registry. See parent module docs.

use std::collections::BTreeSet;

/// Dimensions a PBT's SUT can supply. An invariant lists the *minimum*
/// set its body touches; a `PbtSuiteSpec` listing strictly fewer
/// dimensions filters that invariant out.
///
/// Mirrors `docs/Testing/TESTING_INVARIANT_AUDIT.md`. Keep in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Subsystem {
    /// In-memory block + tree state (no Loro, no Turso).
    BlockTree,
    /// LoroSyncController + Loro doc.
    Loro,
    /// Turso storage + matviews (final state).
    TursoProjection,
    /// CDC stream ordering / delivery semantics (a sub-aspect of
    /// `TursoProjection` called out when an invariant depends on the
    /// stream, not just the final state).
    Cdc,
    /// ReactiveEngine ViewModel tree.
    ViewModel,
    /// Render-expr → ViewModel rendering pipeline (sub-aspect of
    /// `ViewModel` when the renderer specifically is the SUT).
    Renderer,
    /// `InputState` + active-editor mirror.
    EditorState,
    /// Real GPUI/TUI window with a populated `BoundsRegistry`.
    FrontendBounds,
    /// `UserDriver` impl that synthesises interactions.
    Driver,
}

impl Subsystem {
    /// Convenience: every subsystem. The wide PBT (`gpui_ui_pbt`) supplies
    /// all of these; `general_e2e_pbt` supplies all except `FrontendBounds`.
    pub fn all() -> BTreeSet<Subsystem> {
        use Subsystem::*;
        [
            BlockTree,
            Loro,
            TursoProjection,
            Cdc,
            ViewModel,
            Renderer,
            EditorState,
            FrontendBounds,
            Driver,
        ]
        .into_iter()
        .collect()
    }

    /// Subsystems supplied by `general_e2e_pbt` (no real window).
    pub fn headless_wide() -> BTreeSet<Subsystem> {
        let mut s = Self::all();
        s.remove(&Subsystem::FrontendBounds);
        s
    }
}

/// The total `ComponentSet → Subsystem` mapping (ADR 0009 §1). The single place
/// the invariant `Subsystem` selection is *derived* from a component set,
/// replacing the runtime `frontend_geometry.is_some()` branch. Total over all 9
/// `Subsystem` variants — the first ADR draft omitted four, which would have
/// silently stopped checking them.
///
/// Behaviour anchor (migration step 1a): `subsystems(full_headless) ==
/// headless_wide()` and `subsystems(full_gpui) == all()`, so the existing
/// blessed slices' selections are unchanged. Scoped sets (e.g. `loro_vm_fast`)
/// legitimately yield fewer — that is the new, granular capability.
pub fn subsystems(set: &holon_pbt_core::ComponentSet) -> BTreeSet<Subsystem> {
    use holon_pbt_core::{Actor, Projection, StorageAdapter};
    let mut s = BTreeSet::new();
    // Always-on observers: present in every run (the in-memory tree is built
    // and a driver synthesises interactions regardless of backend).
    s.insert(Subsystem::BlockTree);
    s.insert(Subsystem::Driver);
    // ViewModel projection drives the VM tree + the render pipeline.
    if set.has_projection(Projection::ViewModel) {
        s.insert(Subsystem::ViewModel);
        s.insert(Subsystem::Renderer);
    }
    if set.has_projection(Projection::EditorState) {
        s.insert(Subsystem::EditorState);
    }
    // Storage-derived subsystems.
    if set.has_storage(StorageAdapter::Loro) {
        s.insert(Subsystem::Loro);
    }
    if set.has_storage(StorageAdapter::Turso) {
        s.insert(Subsystem::TursoProjection);
        s.insert(Subsystem::Cdc);
    }
    // A real UI window adds the bounds subsystem and selects the GPUI runner.
    if set.has_actor(Actor::UI) {
        s.insert(Subsystem::FrontendBounds);
    }
    s
}

/// Run mode for an invariant. The CDC-lag-tolerant block invariants
/// (`inv-blocks-match-ref/matview`, `inv-watch-rows-match-ref`) must **fail**
/// on every real divergence and only DOWNGRADE the CDC-lag case; that
/// downgrade is modelled as `InvariantResult::Skipped` (orthogonal to
/// `RunMode`), so they are `Strict`. The remaining `Warn` checks are the
/// permanently-Skipped `inv-viewmodel-tree-virtual-slots` and the disabled
/// `inv-blocks-match-ref/loro`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunMode {
    /// Failure terminates the test.
    Strict,
    /// Failure is logged; the run continues. A separate truth-check
    /// (typically a `block_raw` re-query) decides whether the failure
    /// is a flake or a real regression.
    Warn,
}

/// *When*, within a single post-transition tick, the native runner should run
/// an invariant. This is a per-invariant property — some invariants only need
/// to re-check when block data changed, others need a fully-rendered root — but
/// it depends on *runtime* tick state, so (unlike `min_sut`) it can't be a type
/// bound. It is declared here, alongside the invariant's other metadata, and
/// consumed by `run_one`; the dispatch tables carry only the bodies.
///
/// These gates are performance gates, not correctness gates: the bodies
/// themselves return `Skipped` when their precondition is absent, so the gate
/// only avoids paying for an expensive snapshot/interpret that would self-skip
/// anyway. (Irrelevant to the explicit-slice dispatch path, which runs its own
/// chosen invariant list unconditionally.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TickGate {
    /// Every started-app tick.
    #[default]
    Always,
    /// Only when the last transition changed block data (skips nav-only ticks).
    NotNavOnly,
    /// Only when the reference is `is_properly_setup()` (root rendered).
    ProperlySetup,
    /// `ProperlySetup` AND not nav-only.
    ProperlySetupNotNavOnly,
}

impl TickGate {
    pub fn active(self, nav_only: bool, properly_setup: bool) -> bool {
        match self {
            TickGate::Always => true,
            TickGate::NotNavOnly => !nav_only,
            TickGate::ProperlySetup => properly_setup,
            TickGate::ProperlySetupNotNavOnly => properly_setup && !nav_only,
        }
    }
}

/// A runtime override of an invariant's effective [`RunMode`], parsed from the
/// `HOLON_PBT_INVARIANTS` env var. This is the invariant analog of the
/// per-transition `HOLON_PBT_WEIGHTS` knob (`transition_dispatch.rs`): it lets
/// a run **escalate** or **de-escalate** specific invariants without touching
/// the source-of-truth defaults in [`register_default`] (so the
/// `warn_mode_invariants_preserved` guard test still pins the committed set).
///
/// The softening lives only in the environment — the test suite itself is
/// never weakened. Use it to get a disclosed, temporary green run while a real
/// fix is built (Option C in the sort-key-convergence plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModeOverride {
    /// Run the check; a failure terminates the test (force Strict).
    Strict,
    /// Run the check; a failure is logged but does not fail the run.
    Warn,
    /// Do not run the check at all.
    Skip,
}

/// One parsed `pattern:mode` rule. The pattern matches invariant id strings
/// (e.g. `inv-live-children-match-ref`) with a single optional `*` wildcard,
/// case-insensitively — the same glob shape as `WeightPattern` for transition
/// weights, kept local to avoid coupling the two subsystems.
#[derive(Debug, Clone)]
enum IdPattern {
    Exact(String),
    Prefix(String),
    Suffix(String),
    Contains(String),
    Star,
}

impl IdPattern {
    fn parse(raw: &str) -> Self {
        let p = raw.trim().to_ascii_lowercase();
        if p == "*" {
            IdPattern::Star
        } else if let Some(inner) = p.strip_prefix('*').and_then(|s| s.strip_suffix('*')) {
            IdPattern::Contains(inner.to_string())
        } else if let Some(suf) = p.strip_prefix('*') {
            IdPattern::Suffix(suf.to_string())
        } else if let Some(pre) = p.strip_suffix('*') {
            IdPattern::Prefix(pre.to_string())
        } else {
            IdPattern::Exact(p)
        }
    }

    fn matches(&self, id: &str) -> bool {
        let id = id.to_ascii_lowercase();
        match self {
            IdPattern::Star => true,
            IdPattern::Exact(s) => id == *s,
            IdPattern::Prefix(s) => id.starts_with(s),
            IdPattern::Suffix(s) => id.ends_with(s),
            IdPattern::Contains(s) => id.contains(s),
        }
    }
}

fn parse_invariant_overrides() -> Vec<(IdPattern, ModeOverride)> {
    let raw = match std::env::var("HOLON_PBT_INVARIANTS") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => return Vec::new(),
    };
    let mut rules = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((pat, mode)) = entry.split_once(':') else {
            eprintln!("[HOLON_PBT_INVARIANTS] ignoring malformed entry (no ':'): {entry:?}");
            continue;
        };
        let mode = match mode.trim().to_ascii_lowercase().as_str() {
            "strict" => ModeOverride::Strict,
            "warn" => ModeOverride::Warn,
            "skip" => ModeOverride::Skip,
            other => {
                eprintln!(
                    "[HOLON_PBT_INVARIANTS] ignoring entry with unknown mode {other:?} \
                     (expected strict|warn|skip): {entry:?}"
                );
                continue;
            }
        };
        rules.push((IdPattern::parse(pat), mode));
    }
    if !rules.is_empty() {
        // Disclose the active softening loudly — a green run under these
        // overrides is a DISCLOSED degraded run, not a clean pass.
        eprintln!(
            "[HOLON_PBT_INVARIANTS] {} invariant mode override rule(s) active from env",
            rules.len()
        );
    }
    rules
}

/// Effective [`ModeOverride`] for `invariant_id` from `HOLON_PBT_INVARIANTS`,
/// or `None` when no rule matches (use the registry default). First-match-wins
/// in declaration order, mirroring `variant_weight_multiplier`.
pub fn invariant_mode_override(invariant_id: &str) -> Option<ModeOverride> {
    static PARSED: std::sync::OnceLock<Vec<(IdPattern, ModeOverride)>> = std::sync::OnceLock::new();
    let rules = PARSED.get_or_init(parse_invariant_overrides);
    rules
        .iter()
        .find(|(pat, _)| pat.matches(invariant_id))
        .map(|(_, mode)| *mode)
}

/// Stable identifier for one invariant. The string form matches the
/// `[inv-…]` labels emitted by the invariant runner so log greps
/// continue to work.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InvariantId(pub &'static str);

impl InvariantId {
    pub const fn new(s: &'static str) -> Self {
        InvariantId(s)
    }
}

impl std::fmt::Display for InvariantId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Addressable metadata for one invariant. This is metadata only; the
/// executable logic lives in the `Invariant<R, S>` bodies.
#[derive(Debug, Clone)]
pub struct InvariantSpec {
    pub id: InvariantId,
    /// Human description. One line. Long enough to identify the
    /// invariant from a log line without grepping the source.
    pub description: &'static str,
    /// Minimum SUT subsystems the invariant body touches. PBTs with
    /// strictly fewer dimensions filter this invariant out.
    pub min_sut: BTreeSet<Subsystem>,
    pub mode: RunMode,
    /// When the native runner dispatches this invariant within a tick. Defaults
    /// to [`TickGate::Always`]; set per-invariant via [`InvariantSpec::gated`].
    pub gate: TickGate,
}

impl InvariantSpec {
    fn new(
        id: &'static str,
        description: &'static str,
        min_sut: &[Subsystem],
        mode: RunMode,
    ) -> Self {
        InvariantSpec {
            id: InvariantId::new(id),
            description,
            min_sut: min_sut.iter().copied().collect(),
            mode,
            gate: TickGate::Always,
        }
    }

    /// Declare a non-default tick gate (see [`TickGate`]). Chained onto `new`
    /// at the registration site so an invariant's "when" lives beside its
    /// "what it needs" (`min_sut`) and "how it fails" (`mode`).
    fn gated(mut self, gate: TickGate) -> Self {
        self.gate = gate;
        self
    }
}

/// Holds the registered invariants. Build once via [`register_default`].
#[derive(Debug, Default, Clone)]
pub struct InvariantRegistry {
    invariants: Vec<InvariantSpec>,
}

impl InvariantRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, spec: InvariantSpec) {
        assert!(
            !self.invariants.iter().any(|i| i.id == spec.id),
            "duplicate invariant id: {}",
            spec.id
        );
        self.invariants.push(spec);
    }

    pub fn all(&self) -> &[InvariantSpec] {
        &self.invariants
    }

    pub fn len(&self) -> usize {
        self.invariants.len()
    }

    pub fn is_empty(&self) -> bool {
        self.invariants.is_empty()
    }

    pub fn get(&self, id: &InvariantId) -> Option<&InvariantSpec> {
        self.invariants.iter().find(|i| i.id == *id)
    }
}

/// Declares which subsystems a particular PBT entry point's SUT
/// actually supplies. Used to filter the registry: only invariants
/// whose `min_sut` is a subset of `subsystems` apply.
#[derive(Debug, Clone)]
pub struct PbtSuiteSpec {
    pub name: &'static str,
    pub subsystems: BTreeSet<Subsystem>,
}

impl PbtSuiteSpec {
    pub fn new(name: &'static str, subsystems: BTreeSet<Subsystem>) -> Self {
        Self { name, subsystems }
    }

    /// Return the subset of `registry` whose `min_sut` ⊆ `self.subsystems`.
    pub fn select<'a>(&self, registry: &'a InvariantRegistry) -> Vec<&'a InvariantSpec> {
        registry
            .all()
            .iter()
            .filter(|inv| inv.min_sut.is_subset(&self.subsystems))
            .collect()
    }

    /// Convenience: ids of selected invariants.
    pub fn selected_ids(&self, registry: &InvariantRegistry) -> Vec<InvariantId> {
        self.select(registry)
            .into_iter()
            .map(|inv| inv.id.clone())
            .collect()
    }
}

/// Build the canonical registry of all invariants. Metadata for the original
/// 25 derived from `docs/Testing/TESTING_INVARIANT_AUDIT.md`; the storage-slice
/// additions (3) are registered after the 3-subsystem block.
pub fn register_default() -> InvariantRegistry {
    use RunMode::*;
    use Subsystem::*;

    let mut reg = InvariantRegistry::new();

    // ── 1-subsystem invariants ────────────────────────────────────
    reg.register(InvariantSpec::new(
        "inv-loro-no-errors",
        "LoroSyncController must not log any errors.",
        &[Loro],
        Strict,
    ));
    reg.register(
        InvariantSpec::new(
            "inv-frontend-bounds-rendered",
            "BoundsRegistry contains entries for the rendered widget tree.",
            &[ViewModel, FrontendBounds],
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(
        InvariantSpec::new(
            "inv-matview-consistent-with-ref",
            "Root-layout matview carries no ghost rows (ids outside the ref universe).",
            &[TursoProjection],
            // Ghost-only check: a stale root-layout matview row (an id not in the
            // ref universe at all) is a real IVM bug → Strict. Under-projection of
            // content blocks is covered by inv-block-ids-match-ref /
            // inv-live-children-match-ref, so this no longer checks `missing`
            // (root-layout rows vs region content are different hierarchy levels).
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(InvariantSpec::new(
        "inv-sql-budget",
        "SQL operations per step stay within the per-transition budget.",
        &[TursoProjection],
        Strict,
    ));
    reg.register(
        InvariantSpec::new(
            "inv-value-fn-provider-arg-variance-13",
            "value-fn provider arg variance check (issue 13).",
            &[ViewModel],
            Strict,
        )
        .gated(TickGate::NotNavOnly),
    );
    reg.register(
        InvariantSpec::new(
            "inv-value-fn-provider-identity",
            "value-fn provider returns identical results for identical arguments.",
            &[ViewModel],
            Strict,
        )
        .gated(TickGate::NotNavOnly),
    );
    reg.register(
        InvariantSpec::new(
            "inv-viewmodel-editable-text-triggers",
            "Editable-text nodes carry the trigger metadata required for dispatch.",
            &[ViewModel],
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(
        InvariantSpec::new(
            "inv-viewmodel-no-error-widgets",
            "No `Error` widgets in the resolved ViewModel tree.",
            &[ViewModel],
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(
        InvariantSpec::new(
            "inv-viewmodel-snapshot",
            "ViewModel snapshot is present and well-formed.",
            &[ViewModel],
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(
        InvariantSpec::new(
            "inv-viewmodel-tree-virtual-slots",
            "Virtual-slot wiring in the ViewModel tree is consistent.",
            &[ViewModel],
            // Warn to match the body (viewmodel_tree_virtual_slots.rs returns
            // RunMode::Warn and is permanently Skipped — a no-op that never panics).
            Warn,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(
        InvariantSpec::new(
            "inv-frontend-root-not-error",
            "The frontend's root ViewModel node is not an `Error` widget.",
            &[ViewModel],
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );

    // ── 2-subsystem invariants ────────────────────────────────────
    reg.register(InvariantSpec::new(
        "inv-blocks-match-ref/matview",
        "Block-equivalence composite (matview store): the `block` matview / \
         live mirror matches reference; CDC-lag falls back to a `block_raw` \
         truth check → Skipped.",
        &[Loro, TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-blocks-match-ref/loro",
        "Block-equivalence composite (Loro store): the live Loro tree matches \
         reference (non-seed blocks). Strict — seeds now materialize into Loro \
         as Block instances, so a divergence is a real bug. Skipped when Loro \
         is off.",
        &[Loro],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-blocks-match-ref/block_raw",
        "Block-equivalence composite (block_raw store): the write-side \
         `block_raw` table matches reference on {content, properties} \
         (subset — block_raw lacks the junction tags/requires columns). \
         Subsumes the former properties-in-cache check.",
        &[BlockTree, TursoProjection],
        Strict,
    ));
    reg.register(
        InvariantSpec::new(
            "inv-blocks-match-ref/org",
            "Block-equivalence composite (org store): blocks parsed back off the \
         on-disk org files match reference, with per-parent sibling ORDER \
         (disk = renderer-canonical). Subsumes the prior \
         assert_blocks_equivalent + assert_block_order. No Loro gate — org \
         files render from block_raw in both Full and SqlOnly.",
            &[BlockTree, Renderer, TursoProjection],
            Strict,
        )
        .gated(TickGate::NotNavOnly),
    );
    reg.register(
        InvariantSpec::new(
            "inv-editable-text-has-draggable",
            "Each editable-text node is draggable in the rendered window.",
            &[ViewModel, FrontendBounds],
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(InvariantSpec::new(
        "inv-focus-matches-ref",
        "Predicted focused block matches the SUT's actual focus.",
        &[Driver, EditorState],
        Strict,
    ));
    reg.register(
        InvariantSpec::new(
            "inv-window-focus-matches-engine-focus",
            "Committed frame's window-focused editor agrees with the engine's \
             in-memory focused_block (ADR 0010 steal-back / zombie-editor \
             detector). SUT-internal; polled to absorb the spawned-binding lag.",
            &[EditorState, FrontendBounds],
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(InvariantSpec::new(
        "inv-editor-text-matches-ref",
        "SUT's live MutableText for the actively-edited block matches the \
         reference's active_editor_text(). Headless companion to the \
         geometry-gated inv-displayed-text/widget active-editor check; \
         skipped when no editor is active or the text is unobservable.",
        &[EditorState],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-editor-caret-matches-ref",
        "SUT's tracked editor caret byte matches the reference model's \
         active_editor_cursor(). Skipped when no editor is active, the \
         medium can't observe a caret (GPUI InputState), or no keystroke \
         has touched the block since focus.",
        &[Driver, EditorState],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-focus-roots",
        "`focus_roots` matview rows match the reference focus set; CDC-lag \
         (mirror behind matview) → Skipped. A real matview divergence is \
         Strict (CDC-lag is Skipped, orthogonal to RunMode).",
        &[TursoProjection, Cdc],
        Strict,
    ));
    reg.register(
        InvariantSpec::new(
            "inv-frontend-engine",
            "Frontend's own ViewModel resolution has no errors and the \
         expected elements are laid out in the window.",
            &[ViewModel, FrontendBounds],
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(
        InvariantSpec::new(
            "inv-frontend-no-error-widgets",
            "No `Error` widgets in the rendered window.",
            &[ViewModel, FrontendBounds],
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(
        InvariantSpec::new(
            "inv-live-children-match-ref",
            "Live tree children match the reference block-tree structure.",
            // Body reads the Turso `block_raw` projection via `sorted_children`
            // (sut_capabilities.rs `query_sql(... block_raw ORDER BY sort_key)`),
            // so it genuinely needs Turso. The Loro side of the same property is
            // covered by `inv-loro-children-match-ref` under no-Turso wiring.
            &[BlockTree, Loro, TursoProjection],
            Strict,
        )
        .gated(TickGate::NotNavOnly),
    );
    reg.register(
        InvariantSpec::new(
            "inv-loro-children-match-ref",
            "Loro fractional-index sibling order matches the reference document order.",
            &[BlockTree, Loro],
            Strict,
        )
        .gated(TickGate::NotNavOnly),
    );
    reg.register(
        InvariantSpec::new(
            "inv-viewmodel-decompiled-rows-match-query",
            "Decompiled ViewModel rows match the underlying query result.",
            &[ViewModel, TursoProjection],
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(
        InvariantSpec::new(
            "inv-viewmodel-entity-ids-subset-of-data",
            "Entity ids in the ViewModel tree are a subset of the data layer's ids.",
            &[ViewModel, TursoProjection],
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(
        InvariantSpec::new(
            "inv-viewmodel-root-matches-render-expr",
            "Root widget matches the render expression it was produced from.",
            &[ViewModel, Renderer],
            Strict,
        )
        .gated(TickGate::ProperlySetup),
    );
    reg.register(
        InvariantSpec::new(
            "inv-viewmodel-state-toggle-correct",
            "State-toggle wiring resolves to the correct block-side fields.",
            &[ViewModel, BlockTree],
            Strict,
        )
        .gated(TickGate::ProperlySetupNotNavOnly),
    );
    reg.register(InvariantSpec::new(
        "inv-watch-rows-match-ref",
        "Watch CDC stream rows match the reference; CDC-lag → Skipped \
         (a real divergence must fail).",
        &[TursoProjection, Cdc],
        Strict,
    ));

    // ── 3-subsystem invariants ────────────────────────────────────
    // `inv-displayed-text/*` family — same text-equivalence rule at two render
    // layers (see bodies/displayed_text.rs). `/widget` is the on-screen geometry
    // (FrontendBounds, gpui-only); `/viewmodel` is the frontend-agnostic VM tree
    // (ViewModel, runs headless too). A `/widget` fail with `/viewmodel` pass
    // localises the break to the paint/InputState layer.
    reg.register(
        InvariantSpec::new(
            "inv-displayed-text/widget",
            "On-screen text equals the editor-state text for the focused block (else committed content).",
            &[EditorState, ViewModel, FrontendBounds],
            Strict,
        )
        .gated(TickGate::NotNavOnly),
    );
    reg.register(
        InvariantSpec::new(
            "inv-displayed-text/viewmodel",
            "ViewModel-tree `content` prop equals the committed reference content for each \
             rendered block (active editor skipped). Promoted Warn→Strict (Phase 2.6) after \
             the warn period surfaced no event-driven re-render lag on the blessed gates.",
            &[EditorState, ViewModel],
            Strict,
        )
        .gated(TickGate::NotNavOnly),
    );
    reg.register(
        InvariantSpec::new(
            "inv-org-render-fixed-point",
            "Re-rendering the current SQL state produces the same org file (fixed point).",
            // Body renders org from the Turso block cache via
            // `snapshot_org_render_pairs` (`query_sql` + `CacheBlockReader.get_blocks`),
            // so it needs Turso. There is no Loro-sourced render companion, so this
            // property is simply not checked under no-Turso wiring.
            &[BlockTree, Renderer, Loro, TursoProjection],
            Strict,
        )
        .gated(TickGate::NotNavOnly),
    );

    // ── Storage-slice additions ──────────────────────────────────
    reg.register(InvariantSpec::new(
        "inv-block-ids-match-ref",
        "Set of block ids reachable in the SUT's SQL projection equals the reference's non-seed block ids.",
        &[BlockTree, TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-block-content-matches-ref",
        "Per-block `content` column equality between the SQL projection and the reference model (stable ids only; synthetic split/bulk ids skipped).",
        &[BlockTree, TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-block-tags-references-exist",
        "Every block_tags row refers to a block that still exists in block_raw.",
        &[BlockTree, TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-task-state-storage-coherence",
        "Loro task_state and SQL task_state projection agree per block.",
        &[BlockTree, Loro, TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-no-orphan-blocks",
        "Every non-root block in the `block` matview snapshot references a \
         parent that also exists in the snapshot (CDC-lag → Skipped).",
        &[BlockTree, TursoProjection],
        Strict,
    ));

    // ── ADR-0004 Phase 2b — domain invariants ────────────────────────
    //
    // The ADR names four domain invariants. Two are NEW (registered here);
    // the other two are already enforced by existing invariants, so they are
    // documented rather than duplicated:
    //   - "all refs resolve"            → inv-no-orphan-blocks (parent refs) +
    //                                      inv-block-tags-references-exist (tags)
    //   - "children form a valid ordered list"
    //                                   → inv-live-children-match-ref (SQL) +
    //                                      inv-loro-children-match-ref (Loro fi)
    // Both new invariants read the convergent write-side truth (block_raw),
    // so they are domain-tier checks tagged with the existing `BlockTree`
    // subsystem (no dedicated `Domain` variant — `BlockTree` is the in-memory
    // domain tier) plus `TursoProjection` for the snapshot read.
    reg.register(InvariantSpec::new(
        "inv-no-parent-cycles",
        "The block parent relation is acyclic: following parent_id from any \
         block in block_raw terminates at a root without revisiting a node. \
         Complements inv-no-orphan-blocks (parent exists) — together they \
         certify a well-formed forest.",
        &[BlockTree, TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-source-language-iff-source",
        "Every block carries a source_language iff its content_type is Source \
         (Text/Image → None, Source → Some). A domain rule each adapter \
         projection must preserve.",
        &[BlockTree, TursoProjection],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-no-errors",
        "App-runtime error log is empty (no Flutter/event publish errors during \
         initial document sync). General app-error guard, distinct from the \
         component-specific error invariants.",
        &[Cdc],
        Strict,
    ));
    // ── ADR-0004 Phase 4 — per-actor invariant coverage ──────────────
    //
    // The MCP-server actor (MCPServerActorState.active_watches) and the
    // action-engine actor (ActionActorState: app_started, undo/redo stacks,
    // …) were extracted in Phase 4. The ADR named two per-actor invariants;
    // both are documented here rather than added, because:
    //
    //   - MCP "emitted-deltas-correspond-to-domain-changes" is already
    //     enforced: `inv-active-watches-match-ref` compares the SUT's actual
    //     emitted watch streams (`watch_query_ids()`) to the actor's
    //     `active_watch_ids()` (subscription↔emission), and
    //     `inv-watch-rows-match-ref` compares each watch's CDC-delivered rows
    //     to the predicted query result over the current domain — a delta
    //     error surfaces there as a row-set divergence. A third MCP invariant
    //     would be redundant.
    //   - action "undo/redo availability / watcher-cursor monotonicity" is
    //     DEFERRED: the undo subsystem is dormant (ReferenceState::
    //     push_undo_snapshot is a no-op because SqlOperationProvider returns
    //     OperationResult::irreversible() for all ops), so both the ref
    //     undo/redo stacks and the engine's can_undo()/can_redo() are always
    //     empty/false — an availability invariant would be vacuously green.
    //     Add it once the provider produces inverse operations and
    //     push_undo_snapshot is re-enabled. Undo *correctness* meanwhile is
    //     exercised behaviorally via the UndoLastMutation/Redo transitions →
    //     inv-blocks-match-ref. (No watcher-cursor state exists in the PBT.)
    reg.register(InvariantSpec::new(
        "inv-active-watches-match-ref",
        "The set of registered watch query ids on the SUT equals the \
         reference's (subscription-set agreement; watch rows checked by \
         inv-watch-rows-match-ref).",
        &[Cdc],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-view-selection",
        "The SUT's selected view mode equals the reference's (UI \
         view-selection state).",
        &[ViewModel],
        Strict,
    ));
    reg.register(InvariantSpec::new(
        "inv-navigation-focus",
        "The `current_focus` matview's per-region focus matches the \
         reference's navigation focus.",
        &[TursoProjection],
        Strict,
    ));
    reg.register(
        InvariantSpec::new(
            "inv-live-tree-matches-fresh",
            "The persistent live ViewModel tree (collection-driver set_data path) \
         matches a fresh interpretation of the same data rows; divergence means \
         child widgets see stale props in the GPUI frontend. Skipped while the \
         engine/main-panel is still loading.",
            &[ViewModel],
            Strict,
        )
        .gated(TickGate::ProperlySetupNotNavOnly),
    );

    reg
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// ADR 0009 §1 + migration step 1a: the derived `subsystems(set)` mapping
    /// must reproduce today's selection for the existing blessed slices.
    #[test]
    fn subsystems_reproduce_blessed_slice_selection() {
        use holon_pbt_core::ComponentSet;
        // full_headless ≡ general_e2e_pbt (no real window) → headless_wide.
        assert_eq!(
            subsystems(&ComponentSet::full_headless()),
            Subsystem::headless_wide()
        );
        // full_gpui ≡ gpui_ui_pbt (real window) → all 9.
        assert_eq!(subsystems(&ComponentSet::full_gpui()), Subsystem::all());
    }

    /// The total mapping never omits a `Subsystem` that some satisfiable
    /// invariant would need — concretely, `BlockTree` + `Driver` are always on.
    #[test]
    fn subsystems_always_include_intrinsic_observers() {
        use holon_pbt_core::ComponentSet;
        for set in ComponentSet::blessed_sets() {
            let s = subsystems(&set);
            assert!(
                s.contains(&Subsystem::BlockTree),
                "missing BlockTree: {set:?}"
            );
            assert!(s.contains(&Subsystem::Driver), "missing Driver: {set:?}");
        }
    }

    /// ADR 0009 §"Consequences": `subsystems(set)` is monotonic in `set` — a
    /// valid child (one component removed) never *gains* a subsystem. This is
    /// the property bisection's lattice walk relies on.
    #[test]
    fn subsystems_are_monotonic_in_the_set() {
        use holon_pbt_core::ComponentSet;
        for set in ComponentSet::blessed_sets() {
            let parent = subsystems(&set);
            for child in set.valid_children() {
                assert!(
                    subsystems(&child).is_subset(&parent),
                    "subsystems({child:?}) ⊄ subsystems({set:?})"
                );
            }
        }
    }

    /// A scoped set genuinely checks fewer subsystems than the wide ones — the
    /// new granular capability (goal 1).
    #[test]
    fn scoped_set_checks_fewer_subsystems() {
        use holon_pbt_core::ComponentSet;
        let scoped = subsystems(&ComponentSet::loro_vm_fast());
        let wide = subsystems(&ComponentSet::full_headless());
        assert!(scoped.is_subset(&wide));
        assert!(scoped.len() < wide.len());
        // No Turso (loro-only), no EditorState (ViewModel-only), no UI.
        assert!(!scoped.contains(&Subsystem::TursoProjection));
        assert!(!scoped.contains(&Subsystem::EditorState));
        assert!(!scoped.contains(&Subsystem::FrontendBounds));
    }

    /// The gpui_ui_pbt SUT supplies every subsystem; its selection
    /// must include the full registry.
    #[test]
    fn gpui_wide_pbt_selects_all() {
        let reg = register_default();
        let spec = PbtSuiteSpec::new("gpui_ui_pbt", Subsystem::all());
        assert_eq!(spec.select(&reg).len(), reg.len());
    }

    /// The general_e2e_pbt SUT runs headless (no real window), so its selection
    /// must be exactly the registry minus the FrontendBounds-touching
    /// invariants. Derived from `min_sut`, so adding/removing a FrontendBounds
    /// invariant needs no edit here — only a change to the *selection logic*
    /// (or a mis-scoped min_sut) trips it.
    #[test]
    fn headless_wide_pbt_drops_frontend_bounds_invariants() {
        let reg = register_default();
        let spec = PbtSuiteSpec::new("general_e2e_pbt", Subsystem::headless_wide());
        let dropped: Vec<_> = reg
            .all()
            .iter()
            .filter(|inv| inv.min_sut.contains(&Subsystem::FrontendBounds))
            .map(|inv| inv.id.0)
            .collect();
        // Non-vacuous: there *are* FrontendBounds invariants to drop.
        assert!(
            !dropped.is_empty(),
            "no FrontendBounds invariants found — test would pass vacuously"
        );
        // The property: headless selects exactly registry \ {FrontendBounds-touching}.
        assert_eq!(spec.select(&reg).len(), reg.len() - dropped.len());
    }

    /// Phase 5 (T1 editor+Loro PBT): SUT supplies block-tree + loro
    /// + viewmodel + editor-state. Per audit, 11 invariants apply.
    /// This test pins the prediction so a body migration that quietly
    /// widens an invariant's min-SUT triggers a visible review.
    #[test]
    fn phase5_editor_loro_picks_up_expected_count() {
        use Subsystem::*;
        let reg = register_default();
        let spec = PbtSuiteSpec::new(
            "phase5_editor_loro",
            [BlockTree, Loro, ViewModel, EditorState, Renderer]
                .into_iter()
                .collect(),
        );
        let selected = spec.select(&reg);
        // The audit predicts 11; renderer is included here because
        // `inv-viewmodel-root-matches-render-expr` needs it. The block-
        // equivalence composite's `/loro` store ([Loro]) and the ViewModel-only
        // `inv-live-tree-matches-fresh` are also selected here. The two ADR-0004
        // domain invariants are NOT selected — they need TursoProjection, which
        // this slice omits — and for the same reason `inv-live-children-match-ref`
        // and `inv-org-render-fixed-point` are now excluded too (both read the
        // Turso projection, so their min_sut carries TursoProjection). Net
        // selection sits at 14, within the 10..=16 band.
        assert!(
            (10..=16).contains(&selected.len()),
            "phase5 selection size {} outside expected 10..=16 range; \
             check audit drift. Selected: {:?}",
            selected.len(),
            selected.iter().map(|i| i.id.0).collect::<Vec<_>>()
        );
    }

    /// A deliberately under-scoped spec — only ViewModel — must reject
    /// every multi-subsystem invariant. Negative test required by the
    /// Phase 3 exit criteria.
    #[test]
    fn under_scoped_spec_rejects_multi_subsystem() {
        use Subsystem::*;
        let reg = register_default();
        let spec = PbtSuiteSpec::new("viewmodel_only", [ViewModel].into_iter().collect());
        for inv in spec.select(&reg) {
            assert_eq!(
                inv.min_sut.len(),
                1,
                "viewmodel-only spec admitted multi-subsystem invariant {}",
                inv.id
            );
        }
    }

    /// Warn is a deliberate, rare exception: a Warn invariant logs instead of
    /// panicking, so a silent Strict→Warn downgrade weakens the suite (and a
    /// silent Warn→Strict upgrade re-introduces the CDC-lag flakes the Warn path
    /// exists to absorb). This pins the *exact* blessed Warn set — membership is
    /// the property, not a count. Anything that drops out of Warn or sneaks into
    /// it without being added here (with a reason) fails the test.
    ///
    /// Everything not listed here is Strict by design — notably the matview /
    /// watch / focus checks, whose only non-failure path is CDC-lag, modelled as
    /// `InvariantResult::Skipped` (orthogonal to `RunMode`), never as Warn.
    #[test]
    fn warn_mode_invariants_preserved() {
        let reg = register_default();
        let warn: BTreeSet<&str> = reg
            .all()
            .iter()
            .filter(|i| i.mode == RunMode::Warn)
            .map(|i| i.id.0)
            .collect();
        let blessed: BTreeSet<&str> = [
            // Body is permanently Skipped (display_tree blocker) — a no-op that
            // never panics, so Warn is purely cosmetic.
            "inv-viewmodel-tree-virtual-slots",
            // inv-displayed-text/viewmodel promoted Warn→Strict (Phase 2.6).
        ]
        .into_iter()
        .collect();
        assert_eq!(
            warn, blessed,
            "Warn-mode set drifted. Add a deliberate Warn invariant to `blessed` \
             (with a reason) or fix an accidental Strict→Warn downgrade."
        );
    }

    /// H11 anti-rubber-stamp guard (runtime form).
    /// Every invariant a non-wide slice consumes MUST also exist in
    /// the wide registry. The compile-time archlint upgrade is a
    /// future addition; this runtime check catches the regression early.
    ///
    /// Slices currently consume:
    ///   - `cdc_delivery_pbt`: `inv-loro-no-errors`,
    ///     `inv-block-tags-references-exist`
    ///
    /// (`storage_consistency_pbt` was retired — its `storage` preset is now
    /// covered by the convergence harness `subsystem_convergence_pbt`, which
    /// runs the full registry over a generated Turso/Loro wiring.)
    ///
    /// Each id MUST also appear in the wide registry.
    #[test]
    fn storage_slice_invariants_are_subset_of_wide_registry() {
        let reg = register_default();
        let registry_ids: BTreeSet<&str> = reg.all().iter().map(|i| i.id.0).collect();
        let storage_slice_ids: &[&str] = &["inv-loro-no-errors", "inv-block-tags-references-exist"];
        let cdc_slice_ids: &[&str] = &["inv-loro-no-errors", "inv-block-tags-references-exist"];
        for id in storage_slice_ids.iter().chain(cdc_slice_ids.iter()) {
            assert!(
                registry_ids.contains(id),
                "H11 violation: a non-wide slice uses '{id}' but it is not registered in the wide registry"
            );
        }
    }

    /// Every registered id maps to a body file on disk. Derived from the
    /// registry + the filesystem (no hand-maintained mirror), so a registry
    /// entry whose body was deleted/renamed fails fast. Together with
    /// `native_runner_dispatches_exactly_the_registry` (every id is dispatched
    /// somewhere) this covers body↔registry parity without a parallel id list.
    #[test]
    fn every_registry_id_has_a_body_file() {
        use std::path::PathBuf;
        let bodies_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/pbt/invariants/bodies");
        let reg = register_default();
        for inv in reg.all() {
            // id `inv-loro-no-errors` → file `loro_no_errors.rs`. Composite
            // per-store ids carry a `/store` suffix that all map to the one
            // shared body file, e.g. `inv-blocks-match-ref/matview` and
            // `inv-blocks-match-ref/loro` → `blocks_match_ref.rs`.
            let base = inv.id.0.split('/').next().expect("non-empty id");
            let stem = base
                .strip_prefix("inv-")
                .expect("invariant ids start with 'inv-'")
                .replace('-', "_");
            let path = bodies_dir.join(format!("{stem}.rs"));
            assert!(
                path.exists(),
                "registered invariant '{}' is missing body file at {}",
                inv.id,
                path.display()
            );
        }
    }

    /// Reverse direction of `every_registry_id_has_a_body_file`: every body
    /// file must correspond to a registered invariant. Without this, a body
    /// can silently fall out of the registry (and thus out of the
    /// native-vs-slice coverage oracle) — exactly what happened to
    /// `inv-block-content-matches-ref` before Jun 2026.
    #[test]
    fn every_body_file_has_a_registry_entry() {
        use std::collections::BTreeSet;
        use std::path::PathBuf;

        use crate::pbt::composed::composed_invariant_catalog;

        let bodies_dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/pbt/invariants/bodies");

        // The base stem of an invariant id: drop the `/variant` discriminator,
        // strip the `inv-` prefix, and dash→underscore. `inv-block-content-
        // matches-ref/block_raw` → `block_content_matches_ref`.
        let id_to_stem = |id: &str| -> String {
            id.split('/')
                .next()
                .expect("non-empty id")
                .strip_prefix("inv-")
                .expect("invariant ids start with 'inv-'")
                .replace('-', "_")
        };

        // A body is "covered" if it is dispatched by EITHER the native registry
        // OR the composed catalog. The store-variant bodies (`*_backend.rs`,
        // realizing the `/block_raw` SUT-backend check) are composed-only for
        // `block_parent` and native+composed for `block_content` — both are real
        // coverage paths, so the orphan check must consult both registries.
        let mut covered_stems: BTreeSet<String> = register_default()
            .all()
            .iter()
            .map(|inv| id_to_stem(inv.id.0))
            .collect();
        covered_stems.extend(
            composed_invariant_catalog()
                .iter()
                .map(|c| id_to_stem(c.id().0)),
        );

        for entry in std::fs::read_dir(&bodies_dir).expect("read bodies dir") {
            let path = entry.expect("dir entry").path();
            let stem = path
                .file_stem()
                .expect("file stem")
                .to_str()
                .expect("utf-8 stem")
                .to_string();
            if stem == "mod" {
                continue;
            }
            // A `<base>_backend.rs` file realizes the `/block_raw` variant of the
            // base invariant; match it against the base stem.
            let base = stem.strip_suffix("_backend").unwrap_or(&stem);
            assert!(
                covered_stems.contains(&stem) || covered_stems.contains(base),
                "body file {} is dispatched by neither register_default() nor the \
                 composed catalog — wire it (native and/or composed, + NATIVE_ONLY_EXCLUDED \
                 if slice-only) or delete the file",
                path.display(),
            );
        }
    }

    /// Sanity: every invariant's min_sut is non-empty. A zero-subsystem
    /// invariant either has no inputs (suspicious) or wasn't classified
    /// properly.
    #[test]
    fn every_invariant_has_a_non_empty_min_sut() {
        let reg = register_default();
        for inv in reg.all() {
            assert!(
                !inv.min_sut.is_empty(),
                "invariant {} has empty min_sut",
                inv.id
            );
        }
    }
}
