//! Transition: arrow-key navigation from the currently focused block.
//!
//! Mirrors the legacy logic split across `state_machine.rs:673-703` (generator),
//! `state_machine.rs:3181-3183` (precondition),
//! `state_machine.rs:2316-2451` (ref-state apply),
//! `sut.rs:2848-2897` (SUT apply), and
//! `transition_budgets.rs:199-205` (expected SQL).

use crate::pbt::validation::{Reason, check};
use holon_api::Region;
use holon_frontend::navigation::NavDirection;
use proptest::prelude::*;
use proptest::strategy::{BoxedStrategy, Union};
use validated::Validated;

use crate::pbt::reference_state::{CursorPosition, ReferenceState};
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

/// Navigate via arrow keys from the currently focused block in a region.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ArrowNavigate {
    pub region: Region,
    pub direction: NavDirection,
    pub steps: u8,
}

impl TransitionFactory<ReferenceState> for ArrowNavigate {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_frontend::pbt_caps::SutArrowNavigate,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let regions = [Region::Main, Region::LeftSidebar, Region::RightSidebar];
        let mut arms: Vec<(u32, BoxedStrategy<ArrowNavigate>)> = Vec::new();

        for region in &regions {
            if state.ui.tab.focused_entity_id.contains_key(region) {
                // Determine available directions from navigator type
                let render_name = state.active_render_expr_name(*region);
                let directions: Vec<NavDirection> = match render_name.as_deref() {
                    Some("tree") | Some("outline") => {
                        vec![
                            NavDirection::Up,
                            NavDirection::Down,
                            NavDirection::Left,
                            NavDirection::Right,
                        ]
                    }
                    _ => vec![NavDirection::Up, NavDirection::Down],
                };

                let r = region;
                let candidates: Vec<ArrowNavigate> = directions
                    .into_iter()
                    .flat_map(|direction| {
                        (1u8..=3u8).map(move |steps| ArrowNavigate {
                            region: *r,
                            direction,
                            steps,
                        })
                    })
                    .filter(|nav| nav.preconditions(state).is_good())
                    .collect();

                if !candidates.is_empty() {
                    arms.push((1, prop::sample::select(candidates).boxed()));
                }
            }
        }

        check(!arms.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = Union::new_weighted(arms).boxed();
            (1, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for ArrowNavigate {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
            check(
                state.ui.tab.focused_entity_id.contains_key(&self.region),
                Reason::MainFocusNotSet,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        use holon_frontend::navigation::{Boundary, CursorHint};

        let mut current_id = state
            .ui
            .tab
            .focused_entity_id
            .get(&self.region)
            .expect("ArrowNavigate requires focused entity")
            .clone();
        let mut cursor = state
            .ui
            .tab
            .focused_cursor
            .get(&self.region)
            .copied()
            .unwrap_or(CursorPosition::start());

        let navigator = state.build_reference_navigator(self.region);

        for _ in 0..self.steps {
            // Get the content of the currently focused block
            let content = state
                .domain
                .block_state
                .blocks
                .get(&current_id)
                .map(|b| b.content.as_str())
                .unwrap_or("");
            let line_count = if content.is_empty() {
                1
            } else {
                content.split('\n').count()
            };
            let last_line = line_count.saturating_sub(1);

            // Predict whether this arrow causes cross-block navigation
            let crosses_block = match self.direction {
                NavDirection::Up => cursor.line == 0,
                NavDirection::Down => cursor.line >= last_line,
                NavDirection::Left => cursor.line == 0 && cursor.column == 0,
                NavDirection::Right => {
                    let line_len = content
                        .split('\n')
                        .nth(cursor.line)
                        .map(|l| l.len())
                        .unwrap_or(0);
                    cursor.line >= last_line && cursor.column >= line_len
                }
            };

            if crosses_block {
                if let Some(ref nav) = navigator {
                    let boundary = match self.direction {
                        NavDirection::Up => Boundary::Top,
                        NavDirection::Down => Boundary::Bottom,
                        NavDirection::Left => Boundary::Left,
                        NavDirection::Right => Boundary::Right,
                    };
                    let hint = CursorHint {
                        column: cursor.column,
                        boundary,
                    };
                    if let Some(target) = nav.navigate(&current_id, self.direction, &hint) {
                        current_id = target.block_id.clone();
                        // Update cursor from placement
                        let target_content = state
                            .domain
                            .block_state
                            .blocks
                            .get(&current_id)
                            .map(|b| b.content.as_str())
                            .unwrap_or("");
                        let offset = holon_frontend::navigation::placement_to_offset(
                            target_content,
                            target.placement,
                        );
                        let (line, col) =
                            holon_frontend::navigation::offset_to_line_col(target_content, offset);
                        cursor = CursorPosition { line, column: col };
                    }
                    // else: at boundary of collection, stay put
                }
            } else {
                // Intra-block cursor movement
                match self.direction {
                    NavDirection::Up => {
                        cursor.line = cursor.line.saturating_sub(1);
                    }
                    NavDirection::Down => {
                        cursor.line = (cursor.line + 1).min(last_line);
                    }
                    NavDirection::Left => {
                        if cursor.column > 0 {
                            cursor.column -= 1;
                        } else if cursor.line > 0 {
                            cursor.line -= 1;
                            let prev_line_len = content
                                .split('\n')
                                .nth(cursor.line)
                                .map(|l| l.len())
                                .unwrap_or(0);
                            cursor.column = prev_line_len;
                        }
                    }
                    NavDirection::Right => {
                        let line_len = content
                            .split('\n')
                            .nth(cursor.line)
                            .map(|l| l.len())
                            .unwrap_or(0);
                        if cursor.column < line_len {
                            cursor.column += 1;
                        } else if cursor.line < last_line {
                            cursor.line += 1;
                            cursor.column = 0;
                        }
                    }
                }
            }
        }

        // Update focused entity and cursor. Arrow keys change editor
        // focus but NOT navigation — navigation_history is untouched.
        // The global `focused_block` mirror also moves: production
        // GPUI's arrow handler calls `services.set_focus()` on the
        // new target (mirroring what a click would do), so the
        // engine's `UiState.focused_block` follows the per-region
        // pointer.
        state.ui.tab.focused_block = Some(current_id.clone());
        state
            .ui
            .tab
            .focused_entity_id
            .insert(self.region, current_id);
        state.ui.tab.focused_cursor.insert(self.region, cursor);
    }
}

#[allow(async_fn_in_trait)]
impl<S: holon_frontend::pbt_caps::SutArrowNavigate> TransitionImpl<ReferenceState, S>
    for ArrowNavigate
{
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        // The cap's `region` is informational only (the driver emits global
        // arrow keystrokes); the left/right sidebar distinction collapses to
        // `Sidebar`.
        let region = match self.region {
            Region::Main => CapRegion::Main,
            Region::LeftSidebar | Region::RightSidebar => CapRegion::Sidebar,
        };
        sut.apply_arrow_navigate(region, self.direction, self.steps)
            .await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for ArrowNavigate {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS + (self.steps as usize * 2),
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state) + (self.steps as usize * 2),
        }
    }
}
