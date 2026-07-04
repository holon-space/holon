//! `Then` assertions, evaluated against the live SUT.
//!
//! An [`Assertion`] is the assert-side counterpart of an `E2ETransition`. It is
//! evaluated through [`evaluate_assertion`], which takes the inner `E2ESut`
//! concretely (the focus source is the `SutDriver` cap; the widget-text source
//! is `E2ESut`'s inherent headless `widget_tree_*` helpers). The generic runner
//! can't reach these, so a macro-generated `FixtureAssertable` bridge calls this
//! with `&sut.inner` and the shared runtime (see `super::FixtureAssertable`).
//!
//! Widget-text assertions read a **headless** re-interpretation of the block's
//! widget tree (`E2ESut::widget_tree_snapshot` / `widget_tree_for`). These run
//! over the lazily-created headless reactive engine, so the assertion works in a
//! windowless slice. (E3 deleted the `SutRenderer` *capability* off `E2ESut`;
//! these inherent helpers remain solely for this fixture path and are not part
//! of the PBT invariant-composition surface.)
//!
//! Vocabulary v1 (see `matchers::match_assertion`):
//! - `the widget contains "<text>"` / `the widget shows exactly "<text>"`
//! - `block "<id>" contains "<text>"`
//! - `focus is on block "<id>"` / `block "<id>" is focused`
//!
//! Any of these may be prefixed with `within <N> seconds ` to retry until the
//! assertion holds or the timeout elapses — the escape hatch for CDC-lag
//! windows where a read can briefly trail the settled state.
//!
//! Block ids in assertions are reference-model ids: they are resolved through
//! `resolve_ref_block_id` (so `block:ref-doc-0`, a `block::split-N` synthetic,
//! and a stable `:ID:` all work). `WidgetContains`/`FocusOn` need a renderer /
//! focus source; absent both, they fail loud rather than passing vacuously.

use std::time::Duration;

use holon_pbt_core::capabilities::{
    CapRegion, EntityUri, RefFocus, SutFocus, SutRenderer, WidgetSnapshot,
};
use holon_pbt_core::composition::CapMap;

use crate::pbt::op_write_cap::IdResolver;

#[derive(Debug, Clone)]
pub enum Assertion {
    /// The rendered widget tree contains `text`. `locator = None` matches the
    /// root widget; `Some(block_id)` scopes to that block's subtree.
    WidgetContains {
        locator: Option<String>,
        text: String,
        exact: bool,
        within_secs: Option<u64>,
    },
    /// The SUT's focused block resolves to `block_id` (a reference-model id;
    /// remapped through `resolve_ref_block_id`).
    FocusOn {
        block_id: String,
        within_secs: Option<u64>,
    },
}

impl Assertion {
    fn within_secs(&self) -> Option<u64> {
        match self {
            Assertion::WidgetContains { within_secs, .. } => *within_secs,
            Assertion::FocusOn { within_secs, .. } => *within_secs,
        }
    }
}

fn snapshot_text(snap: &WidgetSnapshot) -> String {
    let mut out = String::new();
    for node in snap.walk() {
        if let Some(entity_id) = &node.entity_id {
            out.push_str(entity_id);
            out.push('\n');
        }
        for value in node.props.values() {
            out.push_str(value);
            out.push('\n');
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────────
// Composed (`CapMap`) assertion path — reads the composed cap surface.
// (The `E2ESut` inherent-helper path was deleted with the monolith.)
//
// Both reads the vocabulary needs are already hosted on `full_headless`:
//   - widget tree  → `SutRenderer::widget_tree_snapshot`/`widget_tree_for`
//     (the same headless render pipeline `HeadlessFrontendComponent` runs for
//     the `inv-viewmodel-*` family);
//   - focus        → `SutSqlProjection::current_focus_rows` (the `current_focus`
//     matview, region `"main"`), which `inv-navigation-focus` already proves
//     agrees with the reactive engine each tick.
// Id resolution reuses the harness `IdResolver` (synthetic `block::split-N`
// tails resolved by the per-tick reconcile).
// ─────────────────────────────────────────────────────────────────────────

/// Resolve a reference-model id into SUT id space via the harness `IdResolver`
/// (identity on miss — a stable `:ID:` maps to itself).
fn resolve_via(resolver: &IdResolver, id: &EntityUri) -> EntityUri {
    resolver
        .lock()
        .expect("IdResolver lock")
        .get(id)
        .cloned()
        .unwrap_or_else(|| id.clone())
}

/// Evaluate an [`Assertion`] against the reference state and a composed `CapMap`.
/// Retries under a `within N seconds` budget; reads the composed cap surface —
/// the bridge `ComposedSut::evaluate_assert` calls.
pub async fn evaluate_assertion_caps<R>(
    assertion: &Assertion,
    ref_: &R,
    caps: &CapMap,
    resolver: &IdResolver,
) -> Result<(), String>
where
    R: RefFocus,
{
    let deadline = assertion
        .within_secs()
        .map(|secs| tokio::time::Instant::now() + Duration::from_secs(secs));

    loop {
        let result = match assertion {
            Assertion::WidgetContains {
                locator,
                text,
                exact,
                ..
            } => widget_contains_caps(caps, resolver, locator.as_deref(), text, *exact).await,
            Assertion::FocusOn { block_id, .. } => {
                focus_on_caps(ref_, caps, resolver, block_id).await
            }
        };
        match result {
            Ok(()) => return Ok(()),
            Err(msg) => match deadline {
                Some(d) if tokio::time::Instant::now() < d => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                _ => return Err(msg),
            },
        }
    }
}

async fn widget_contains_caps(
    caps: &CapMap,
    resolver: &IdResolver,
    locator: Option<&str>,
    text: &str,
    exact: bool,
) -> Result<(), String> {
    let (scope, haystack) = match locator {
        None => (
            "root widget".to_string(),
            snapshot_text(&caps.widget_tree_snapshot().await),
        ),
        Some(id) => {
            let id_uri = EntityUri::parse(id).map_err(|e| {
                format!("[widget-contains] locator {id:?} is not a valid EntityUri: {e}")
            })?;
            let resolved = resolve_via(resolver, &id_uri);
            let snap = caps.widget_tree_for(&resolved).await.ok_or_else(|| {
                format!("[widget-contains] block {id:?} (resolved {resolved:?}) did not render (no widget tree)")
            })?;
            (format!("block {id:?}"), snapshot_text(&snap))
        }
    };

    let matched = if exact {
        haystack.trim() == text.trim()
    } else {
        haystack.contains(text)
    };
    if matched {
        return Ok(());
    }
    let qualifier = if exact { "exactly " } else { "" };
    Err(format!(
        "[widget-contains] expected {scope} to contain {qualifier}{text:?}, but rendered text was:\n{haystack}"
    ))
}

async fn focus_on_caps<R: RefFocus>(
    ref_: &R,
    caps: &CapMap,
    resolver: &IdResolver,
    block_id: &str,
) -> Result<(), String> {
    let block_id_uri = EntityUri::parse(block_id)
        .map_err(|e| format!("[focus-on] block id {block_id:?} is not a valid EntityUri: {e}"))?;
    let expected = resolve_via(resolver, &block_id_uri);

    // The composed full_headless SUT's focus source is the `current_focus`
    // matview (region "main"); `inv-navigation-focus` (required every tick)
    // proves it agrees with the reactive engine, so a single read suffices.
    let rows = caps.current_focus_rows().await;
    let sut_focus = rows
        .into_iter()
        .find(|(region, _)| region == "main")
        .and_then(|(_, block)| block)
        .map(|s| EntityUri::parse(&s).expect("current_focus.block_id must be a valid EntityUri"));

    match sut_focus {
        Some(focus) if focus == expected || focus.as_str() == block_id => Ok(()),
        other => {
            let ref_focus = ref_.current_focus(CapRegion::Main);
            Err(format!(
                "[focus-on] expected focus on {block_id:?} (resolved {expected:?}), but composed \
                 current_focus(main) = {other:?} (reference model focus = {ref_focus:?})"
            ))
        }
    }
}
