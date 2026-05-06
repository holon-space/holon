//! Engine-vs-GPUI panic-time diagnostic.
//!
//! When `inv-displayed-text` collects mismatches between the GPUI-rendered
//! string and the reference model's expected string, this helper asks the
//! shared `ReactiveEngine` what *it* thinks the widget should render. The
//! resulting tag tells you whether to look at the backend or the GPUI render
//! layer:
//!
//! - `EngineAlsoStale` — engine produces the same wrong text. Bug is upstream
//!   of GPUI (ReactiveViewModel / matview / CDC / SQL).
//! - `GpuiRenderOnly`  — engine produces the expected text. GPUI render layer
//!   failed to apply the engine's update (text widget missed its data signal,
//!   InputState skipped a refresh, etc.).
//! - `EngineThirdValue` — engine produces a third string, neither the on-screen
//!   nor the expected one. Rare; flagged separately so it doesn't get lumped
//!   into either bucket.
//! - `NoEngineWidget` — engine snapshot has no matching `editable_text` /
//!   `text` widget for the block (e.g. a sidebar `text(col(...))` whose
//!   widget is rendered by the *parent's* render expression, not the block's
//!   own). Diagnostic doesn't apply.
//!
//! The diagnostic is intentionally read-only and non-blocking: it runs once
//! while `inv-displayed-text` is building its panic message, then proptest
//! shrinks the failing case as before.

use std::sync::Arc;

use holon_api::EntityUri;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_frontend::view_model::{ViewKind, ViewModel};

/// Result of comparing one (block, on_screen, expected) triple to the
/// engine's snapshot.
#[derive(Debug, Clone)]
pub enum DiagTag {
    EngineAlsoStale,
    GpuiRenderOnly,
    EngineThirdValue(String),
    NoEngineWidget,
}

impl DiagTag {
    /// Short human-readable form for embedding in a panic message.
    pub fn as_label(&self) -> String {
        match self {
            DiagTag::EngineAlsoStale => {
                "engine snapshot also stale → backend (ViewModel/matview/CDC)".into()
            }
            DiagTag::GpuiRenderOnly => {
                "engine snapshot matches expected → GPUI render layer".into()
            }
            DiagTag::EngineThirdValue(v) => {
                format!("engine snapshot has third value {v:?} → investigate engine independently")
            }
            DiagTag::NoEngineWidget => {
                "no matching engine widget for this block (sidebar text? wrong scheme?)".into()
            }
        }
    }
}

/// Inspect the engine's snapshot for `block_id` and classify the mismatch.
pub fn diagnose_displayed_text(
    engine: &Arc<ReactiveEngine>,
    block_id: &str,
    on_screen: &str,
    expected: &str,
) -> DiagTag {
    let Ok(uri) = EntityUri::parse(block_id) else {
        return DiagTag::NoEngineWidget;
    };
    // `snapshot_resolved` (BuilderServices) recursively resolves nested
    // LiveBlocks — needed when the block's editable_text is inside a tree
    // / outline / table-row produced by an ancestor's render expression.
    let snapshot = engine.snapshot_resolved(&uri);
    let Some(content) = first_text_for_block(&snapshot, block_id) else {
        return DiagTag::NoEngineWidget;
    };
    if content == on_screen {
        DiagTag::EngineAlsoStale
    } else if content == expected {
        DiagTag::GpuiRenderOnly
    } else {
        DiagTag::EngineThirdValue(content)
    }
}

/// Walk the engine's ViewModel snapshot and return the first
/// `EditableText` or `Text` content whose entity id matches `target_id`.
///
/// Mirrors how GPUI's geometry registry attaches `entity_id` to text-bearing
/// widgets — comparing the engine's leaf content to the on-screen string is
/// the same comparison `inv-displayed-text` does, just at the engine level.
fn first_text_for_block(vm: &ViewModel, target_id: &str) -> Option<String> {
    if vm.entity_id().map_or(false, |id| id == target_id) {
        match &vm.kind {
            ViewKind::EditableText { content, .. } => return Some(content.clone()),
            ViewKind::Text { content, .. } => return Some(content.clone()),
            _ => {}
        }
    }
    for child in vm.children() {
        if let Some(c) = first_text_for_block(child, target_id) {
            return Some(c);
        }
    }
    None
}
