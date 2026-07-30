//! Onboarding tours — the frontend-agnostic core (spike).
//!
//! See `docs/Proposals/OnboardingTours-2026-07-12.md`. A tour is an ordinary
//! block subtree: the root carries the `Tour` tag + tour-level props, each
//! child is a step carrying `TOUR_ANCHOR` / `TOUR_ADVANCE` props and `content`
//! as copy.
//!
//! This module owns the *typed core* — parse-at-the-boundary of the raw drawer
//! strings into [`Tour`], anchor→rect resolution against the same
//! [`GeometryProvider`] the real GPUI `BoundsRegistry` implements, and the
//! projection a `TourViewModel` exposes to a view. It holds **no** view logic
//! (no GPUI types) and does **no** engine I/O: the observed-advance predicate
//! is evaluated from counts the engine supplies, so the same code drives every
//! frontend and every test rung.

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::block::Block;

use crate::geometry::GeometryProvider;

/// A resolved on-screen rectangle (subset of `ElementInfo`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Well-known UI regions a step can point at. These need stable ids registered
/// in the geometry registry — the G1 gap in the proposal (regions are render
/// context enums today, not recorded bounds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WellKnownPanel {
    Sidebar,
    Main,
}

impl WellKnownPanel {
    fn parse(s: &str) -> Result<Self> {
        match s {
            "sidebar" => Ok(Self::Sidebar),
            "main" => Ok(Self::Main),
            other => bail!("unknown well-known panel {other:?} (expected sidebar|main)"),
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Sidebar => "sidebar",
            Self::Main => "main",
        }
    }
}

/// Where a step points. Parsed from `TOUR_ANCHOR`.
#[derive(Debug, Clone, PartialEq)]
pub enum AnchorSelector {
    /// A specific block occurrence — geometry key `render-entity-{id}`.
    Block(EntityUri),
    /// Any occurrence of an entity — resolved via the recorded `entity_id`.
    Entity(EntityUri),
    /// A UI region — geometry key `panel:{name}` (G1: not yet recorded).
    Panel(WellKnownPanel),
}

impl AnchorSelector {
    /// Parse `block:<id>` / `entity:<id>` / `panel:<name>`. Fails loudly on an
    /// unknown scheme — a malformed anchor must never silently disable a step.
    pub fn parse(raw: &str) -> Result<Self> {
        let (scheme, rest) = raw
            .split_once(':')
            .with_context(|| format!("anchor {raw:?} missing scheme (block:/entity:/panel:)"))?;
        match scheme {
            "block" => Ok(Self::Block(EntityUri::from_raw(rest))),
            "entity" => Ok(Self::Entity(EntityUri::from_raw(rest))),
            "panel" => Ok(Self::Panel(WellKnownPanel::parse(rest)?)),
            other => bail!("anchor {raw:?} has unknown scheme {other:?}"),
        }
    }

    /// The `BoundsRegistry` key this selector resolves against, for the
    /// id-keyed lookups (`Block`/`Panel`). `Entity` resolves by scanning
    /// `entity_id` instead (see [`resolve_anchor`]).
    pub fn geometry_key(&self) -> Option<String> {
        match self {
            Self::Block(id) => Some(format!("render-entity-{}", id.id())),
            Self::Panel(p) => Some(format!("panel:{}", p.as_str())),
            Self::Entity(_) => None,
        }
    }
}

/// A predicate over engine state the tour subscribes to for a gated step.
/// Evaluated from engine-supplied observations — never by polling the view.
#[derive(Debug, Clone, PartialEq)]
pub enum StatePredicate {
    /// "The user created a block under this anchor." Satisfied once the child
    /// count under the anchor exceeds the count captured when the step opened.
    ChildCreatedUnder { under: AnchorSelector },
}

impl StatePredicate {
    /// Given the child count captured when the step became active (`baseline`)
    /// and the current count, has the gate opened?
    pub fn satisfied(&self, baseline: usize, current: usize) -> bool {
        match self {
            Self::ChildCreatedUnder { .. } => current > baseline,
        }
    }

    /// The anchor whose children the engine must count for this predicate.
    pub fn observed_anchor(&self) -> &AnchorSelector {
        match self {
            Self::ChildCreatedUnder { under } => under,
        }
    }
}

/// When a step advances. Parsed from `TOUR_ADVANCE`.
#[derive(Debug, Clone, PartialEq)]
pub enum AdvanceCondition {
    /// Manual — the user clicks "Next".
    Next,
    /// Gated — advances when a predicate over engine state holds.
    Observed(StatePredicate),
}

impl AdvanceCondition {
    /// Grammar: `next` | `observed:child-created-under(<anchor>)`.
    pub fn parse(raw: &str) -> Result<Self> {
        let raw = raw.trim();
        if raw == "next" {
            return Ok(Self::Next);
        }
        let body = raw
            .strip_prefix("observed:")
            .with_context(|| format!("advance {raw:?} not `next` or `observed:...`"))?;
        let inner = body
            .strip_prefix("child-created-under(")
            .and_then(|s| s.strip_suffix(')'))
            .with_context(|| {
                format!("observed advance {body:?} not `child-created-under(<anchor>)`")
            })?;
        let under = AnchorSelector::parse(inner)?;
        Ok(Self::Observed(StatePredicate::ChildCreatedUnder { under }))
    }
}

/// One tour step, parsed from a step block.
#[derive(Debug, Clone, PartialEq)]
pub struct TourStep {
    pub id: EntityUri,
    pub copy: String,
    pub anchor: AnchorSelector,
    pub advance: AdvanceCondition,
}

/// A tour, parsed from its root + step subtree.
#[derive(Debug, Clone, PartialEq)]
pub struct Tour {
    pub id: EntityUri,
    pub steps: Vec<TourStep>,
}

/// Case-insensitive drawer-property lookup (org stores keys in their authored
/// case, e.g. `TOUR_ANCHOR`).
fn prop<'a>(block: &'a Block, key: &str) -> Option<&'a str> {
    block
        .properties
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, v)| match v {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        })
}

/// Parse a tour from its root block and its ordered step blocks (already in
/// sibling/`sort_key` order). Every malformed prop is a loud error — a tour
/// that cannot be fully parsed does not load.
pub fn parse_tour(root: &Block, steps: &[Block]) -> Result<Tour> {
    if steps.is_empty() {
        bail!("tour {} has no steps", root.id);
    }
    let parsed = steps
        .iter()
        .map(|s| {
            let anchor_raw = prop(s, "TOUR_ANCHOR")
                .with_context(|| format!("step {} missing TOUR_ANCHOR", s.id))?;
            let advance_raw = prop(s, "TOUR_ADVANCE")
                .with_context(|| format!("step {} missing TOUR_ADVANCE", s.id))?;
            Ok(TourStep {
                id: s.id.clone(),
                copy: s.content.clone(),
                anchor: AnchorSelector::parse(anchor_raw)
                    .with_context(|| format!("step {} anchor", s.id))?,
                advance: AdvanceCondition::parse(advance_raw)
                    .with_context(|| format!("step {} advance", s.id))?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(Tour {
        id: root.id.clone(),
        steps: parsed,
    })
}

/// Result of resolving an anchor against the live geometry. `Missing` is the
/// fail-loud signal (the view shows a degraded-mode banner) — never a silent
/// skip.
#[derive(Debug, Clone, PartialEq)]
pub enum AnchorResolution {
    Resolved(Rect),
    Missing,
}

/// Resolve a step anchor to an on-screen rect via the same `GeometryProvider`
/// the real GPUI `BoundsRegistry` implements.
pub fn resolve_anchor(anchor: &AnchorSelector, geo: &dyn GeometryProvider) -> AnchorResolution {
    let info = match anchor {
        AnchorSelector::Entity(id) => geo.find_by_entity_id(id.id()),
        _ => anchor.geometry_key().and_then(|key| geo.element_info(&key)),
    };
    match info {
        Some(i) => AnchorResolution::Resolved(Rect {
            x: i.x,
            y: i.y,
            width: i.width,
            height: i.height,
        }),
        None => AnchorResolution::Missing,
    }
}

/// The projection a view reads — one active step, no tour logic leaks to the
/// view.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveStepView {
    pub index: usize,
    pub total: usize,
    pub copy: String,
    pub anchor: AnchorSelector,
    pub advance: AdvanceCondition,
}

/// The session-level tour projection (spike form).
///
/// Production wraps `active_step` in a `Mutable<usize>` derived from a
/// persisted progress record and installs a real wait-for subscription for the
/// active step's [`StatePredicate`] (proposal §5, gaps G3/G4). The spike keeps
/// the cursor plain and lets the caller drive advancement so the *shape* is
/// exercised without the reactive/subscription plumbing.
#[derive(Debug, Clone)]
pub struct TourViewModel {
    tour: Tour,
    active_step: usize,
    /// Child count under the active step's observed anchor, captured on entry.
    /// `None` for a `Next` step.
    observed_baseline: Option<usize>,
}

impl TourViewModel {
    pub fn new(tour: Tour) -> Self {
        Self {
            tour,
            active_step: 0,
            observed_baseline: None,
        }
    }

    pub fn active_index(&self) -> usize {
        self.active_step
    }

    pub fn is_finished(&self) -> bool {
        self.active_step >= self.tour.steps.len()
    }

    pub fn active_step_view(&self) -> Option<ActiveStepView> {
        let step = self.tour.steps.get(self.active_step)?;
        Some(ActiveStepView {
            index: self.active_step,
            total: self.tour.steps.len(),
            copy: step.copy.clone(),
            anchor: step.anchor.clone(),
            advance: step.advance.clone(),
        })
    }

    fn active(&self) -> Option<&TourStep> {
        self.tour.steps.get(self.active_step)
    }

    /// Record the observed baseline for a gated step when it becomes active.
    /// Production does this from the wait-for subscription's initial read.
    pub fn arm_observation(&mut self, current_child_count: usize) {
        self.observed_baseline = Some(current_child_count);
    }

    /// Whether the active step's gate is open, given the current child count
    /// under its observed anchor. `false` for a `Next` step (never
    /// auto-advances).
    pub fn observed_gate_open(&self, current_child_count: usize) -> bool {
        match self.active().map(|s| &s.advance) {
            Some(AdvanceCondition::Observed(pred)) => {
                let baseline = self.observed_baseline.unwrap_or(current_child_count);
                pred.satisfied(baseline, current_child_count)
            }
            _ => false,
        }
    }

    /// Advance to the next step. In production this is dispatched as an op with
    /// `OpOrigin::User` so it persists/syncs/undoes — the spike moves the
    /// cursor directly to prove the projection, and the directed test
    /// separately proves the op round-trip.
    pub fn advance(&mut self) {
        if self.active_step < self.tour.steps.len() {
            self.active_step += 1;
            self.observed_baseline = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::*;
    use crate::geometry::ElementInfo;

    /// Minimal in-memory `GeometryProvider` — same trait the real GPUI
    /// `BoundsRegistry` implements, so the anchor→rect seam is exercised for
    /// real.
    #[derive(Clone)]
    struct MockGeometry(HashMap<String, ElementInfo>);

    impl GeometryProvider for MockGeometry {
        fn element_info(&self, id: &str) -> Option<ElementInfo> {
            self.0.get(id).cloned()
        }
        fn all_elements(&self) -> Vec<(String, ElementInfo)> {
            self.0.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        }
        fn clone_box(&self) -> Box<dyn GeometryProvider> {
            Box::new(self.clone())
        }
    }

    fn info(entity: Option<&str>) -> ElementInfo {
        ElementInfo {
            x: 10.0,
            y: 20.0,
            width: 100.0,
            height: 40.0,
            widget_type: Arc::from("panel"),
            entity_id: entity.map(Arc::from),
            has_content: true,
            parent_id: None,
            displayed_text: None,
            focused: None,
            styled_runs: None,
            opacity: None,
            expected_size: Default::default(),
        }
    }

    fn step_block(id: &str, content: &str, anchor: &str, advance: &str) -> Block {
        let mut b = Block {
            id: EntityUri::from_raw(id),
            content: content.to_string(),
            ..Default::default()
        };
        b.set_property("TOUR_ANCHOR", Value::String(anchor.to_string()));
        b.set_property("TOUR_ADVANCE", Value::String(advance.to_string()));
        b
    }

    #[test]
    fn parses_a_three_step_tour() {
        let root = Block {
            id: EntityUri::from_raw("tour-welcome"),
            ..Default::default()
        };
        let steps = vec![
            step_block("s1", "sidebar", "panel:sidebar", "next"),
            step_block("s2", "main", "panel:main", "next"),
            step_block(
                "s3",
                "create one",
                "panel:main",
                "observed:child-created-under(block:page-x)",
            ),
        ];
        let tour = parse_tour(&root, &steps).unwrap();
        assert_eq!(tour.steps.len(), 3);
        assert_eq!(
            tour.steps[0].anchor,
            AnchorSelector::Panel(WellKnownPanel::Sidebar)
        );
        assert_eq!(tour.steps[1].advance, AdvanceCondition::Next);
        assert_eq!(
            tour.steps[2].advance,
            AdvanceCondition::Observed(StatePredicate::ChildCreatedUnder {
                under: AnchorSelector::Block(EntityUri::from_raw("page-x")),
            })
        );
    }

    #[test]
    fn malformed_advance_fails_loudly() {
        let root = Block {
            id: EntityUri::from_raw("t"),
            ..Default::default()
        };
        let steps = vec![step_block(
            "s1",
            "x",
            "panel:sidebar",
            "when-i-feel-like-it",
        )];
        let err = parse_tour(&root, &steps).unwrap_err();
        assert!(format!("{err:#}").contains("advance"), "got: {err:#}");
    }

    #[test]
    fn unknown_panel_fails_loudly() {
        assert!(AnchorSelector::parse("panel:nope").is_err());
    }

    #[test]
    fn resolves_panel_and_block_anchors_and_reports_missing() {
        let mut m = HashMap::new();
        m.insert("panel:sidebar".to_string(), info(None));
        m.insert("render-entity-page-x".to_string(), info(Some("page-x")));
        let geo = MockGeometry(m);

        let sidebar = AnchorSelector::Panel(WellKnownPanel::Sidebar);
        assert_eq!(
            resolve_anchor(&sidebar, &geo),
            AnchorResolution::Resolved(Rect {
                x: 10.0,
                y: 20.0,
                width: 100.0,
                height: 40.0
            })
        );

        let block = AnchorSelector::Block(EntityUri::from_raw("page-x"));
        assert!(matches!(
            resolve_anchor(&block, &geo),
            AnchorResolution::Resolved(_)
        ));

        // Entity anchor resolved by scanning recorded entity_id.
        let entity = AnchorSelector::Entity(EntityUri::from_raw("page-x"));
        assert!(matches!(
            resolve_anchor(&entity, &geo),
            AnchorResolution::Resolved(_)
        ));

        // Fail loud, not skip.
        let missing = AnchorSelector::Panel(WellKnownPanel::Main);
        assert_eq!(resolve_anchor(&missing, &geo), AnchorResolution::Missing);
    }

    #[test]
    fn view_model_projects_and_advances_with_gating() {
        let root = Block {
            id: EntityUri::from_raw("t"),
            ..Default::default()
        };
        let steps = vec![
            step_block("s1", "sidebar", "panel:sidebar", "next"),
            step_block(
                "s2",
                "create one",
                "panel:main",
                "observed:child-created-under(block:page-x)",
            ),
        ];
        let mut vm = TourViewModel::new(parse_tour(&root, &steps).unwrap());

        // Step 1: manual. Its gate never opens on child count.
        let v = vm.active_step_view().unwrap();
        assert_eq!((v.index, v.total), (0, 2));
        assert!(!vm.observed_gate_open(999));
        vm.advance();

        // Step 2: gated. Arm baseline, gate opens only once a child appears.
        assert_eq!(vm.active_index(), 1);
        vm.arm_observation(3);
        assert!(!vm.observed_gate_open(3), "no new child yet");
        assert!(vm.observed_gate_open(4), "one child created");
        vm.advance();
        assert!(vm.is_finished());
    }
}
