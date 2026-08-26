//! `Then` assertions, evaluated against the live SUT.
//!
//! An [`Assertion`] is the assert-side counterpart of an `E2ETransition`. It is
//! evaluated through [`evaluate_assertion`], which takes the inner `E2ESut`
//! concretely (the focus source is the `SutDriver` cap; the widget-text source
//! is `E2ESut`'s inherent headless `widget_tree_*` helpers). The generic runner
//! can't reach these, so a macro-generated `FixtureAssertable` bridge calls
//! this with `&sut.inner` and the shared runtime (see
//! `super::FixtureAssertable`).
//!
//! Widget-text assertions read a **headless** re-interpretation of the block's
//! widget tree (`E2ESut::widget_tree_snapshot` / `widget_tree_for`). These run
//! over the lazily-created headless reactive engine, so the assertion works in
//! a windowless slice. (E3 deleted the `SutRenderer` *capability* off `E2ESut`;
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

use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutEditorMirrorRead;
use holon_pbt_core::capabilities::SutFocus;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::capabilities::WidgetSnapshot;
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
    /// The rendered widget tree does NOT contain `text` — the inverse of
    /// [`Assertion::WidgetContains`], for pinning chrome Holon deliberately
    /// does not draw. Carries no `within` budget on purpose: an absence is
    /// true the instant it is read, so retrying could only mask a
    /// still-arriving render. Order it AFTER the positive assertions that
    /// prove the surface has settled.
    WidgetOmits {
        locator: Option<String>,
        text: String,
    },
    /// No `ViewKind::Error` widget anywhere in the scope. The rendered failure
    /// of a widget is otherwise INVISIBLE to a text assertion — worse, an
    /// error message quoting the thing that failed (a query's own SQL, say)
    /// can satisfy a `contains` that was meant to prove the healthy render.
    /// Pair this with any assertion whose expected text could appear inside a
    /// failure message. Budget-free for the same reason as
    /// [`Assertion::WidgetOmits`].
    NoErrorWidget { locator: Option<String> },
    /// `block_id`'s open slash-command menu offers (or does not offer) an item
    /// labelled `label`. Reads
    /// `SutEditorMirrorRead::editor_slash_menu_labels`.
    SlashMenuOffers {
        block_id: String,
        label: String,
        expected: bool,
        within_secs: Option<u64>,
    },
    /// The SUT's focused block resolves to `block_id` (a reference-model id;
    /// remapped through `resolve_ref_block_id`).
    FocusOn {
        block_id: String,
        within_secs: Option<u64>,
    },
    /// `child_id`'s parent in the SUT store is `parent_id`. Both are
    /// reference-model ids (remapped through the harness `IdResolver`).
    ParentIs {
        child_id: String,
        parent_id: String,
        within_secs: Option<u64>,
    },
    /// `block_id` sits at 1-based position `index` among `parent_id`'s children
    /// in `sort_key` order.
    ChildIndex {
        block_id: String,
        index: usize,
        parent_id: String,
        within_secs: Option<u64>,
    },
    /// `block_id` sorts after `other_id` among their common parent's children.
    ComesAfter {
        block_id: String,
        other_id: String,
        within_secs: Option<u64>,
    },
    /// `block_id`'s stored task-state keyword. `expected = None` asserts the
    /// block is not a task.
    TaskState {
        block_id: String,
        expected: Option<String>,
        within_secs: Option<u64>,
    },
    /// `block_id`'s persisted `collapsed` flag in the write-side store.
    Collapsed {
        block_id: String,
        expected: bool,
        within_secs: Option<u64>,
    },
    /// `block_id`'s `block_links` row for `target` resolves to `resolved_id`.
    /// `resolved_id = None` asserts the link dangles.
    LinkResolves {
        block_id: String,
        target: String,
        resolved_id: Option<String>,
        within_secs: Option<u64>,
    },
}

impl Assertion {
    fn within_secs(&self) -> Option<u64> {
        match self {
            Assertion::WidgetContains { within_secs, .. } => *within_secs,
            Assertion::WidgetOmits { .. } => None,
            Assertion::NoErrorWidget { .. } => None,
            Assertion::SlashMenuOffers { within_secs, .. } => *within_secs,
            Assertion::FocusOn { within_secs, .. } => *within_secs,
            Assertion::ParentIs { within_secs, .. } => *within_secs,
            Assertion::ChildIndex { within_secs, .. } => *within_secs,
            Assertion::ComesAfter { within_secs, .. } => *within_secs,
            Assertion::TaskState { within_secs, .. } => *within_secs,
            Assertion::Collapsed { within_secs, .. } => *within_secs,
            Assertion::LinkResolves { within_secs, .. } => *within_secs,
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
//   - widget tree  → `SutRenderer::widget_tree_snapshot`/`widget_tree_for` (the
//     same headless render pipeline `HeadlessFrontendComponent` runs for the
//     `inv-viewmodel-*` family);
//   - focus        → `SutSqlProjection::current_focus_rows` (the
//     `current_focus` matview, region `"main"`), which `inv-navigation-focus`
//     already proves agrees with the reactive engine each tick.
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

/// Evaluate an [`Assertion`] against the reference state and a composed
/// `CapMap`. Retries under a `within N seconds` budget; reads the composed cap
/// surface — the bridge `ComposedSut::evaluate_assert` calls.
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
            Assertion::WidgetOmits { locator, text } => {
                widget_omits_caps(caps, resolver, locator.as_deref(), text).await
            }
            Assertion::NoErrorWidget { locator } => {
                no_error_widget_caps(caps, resolver, locator.as_deref()).await
            }
            Assertion::SlashMenuOffers {
                block_id,
                label,
                expected,
                ..
            } => slash_menu_offers_caps(caps, resolver, block_id, label, *expected).await,
            Assertion::FocusOn { block_id, .. } => {
                focus_on_caps(ref_, caps, resolver, block_id).await
            }
            Assertion::ParentIs {
                child_id,
                parent_id,
                ..
            } => parent_is_caps(caps, resolver, child_id, parent_id).await,
            Assertion::ChildIndex {
                block_id,
                index,
                parent_id,
                ..
            } => child_index_caps(caps, resolver, block_id, *index, parent_id).await,
            Assertion::ComesAfter {
                block_id, other_id, ..
            } => comes_after_caps(caps, resolver, block_id, other_id).await,
            Assertion::TaskState {
                block_id, expected, ..
            } => task_state_caps(caps, resolver, block_id, expected.as_deref()).await,
            Assertion::Collapsed {
                block_id, expected, ..
            } => collapsed_caps(caps, resolver, block_id, *expected).await,
            Assertion::LinkResolves {
                block_id,
                target,
                resolved_id,
                ..
            } => link_resolves_caps(caps, resolver, block_id, target, resolved_id.as_deref()).await,
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
    let (scope, haystack) = widget_haystack(caps, resolver, locator).await?;

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
        "[widget-contains] expected {scope} to contain {qualifier}{text:?}, but rendered text \
         was:\n{haystack}"
    ))
}

/// The `(scope-label, rendered-text)` pair a widget assertion matches against.
/// `locator = None` is the whole tree; `Some(id)` scopes to that block's
/// subtree.
async fn widget_scope(
    caps: &CapMap,
    resolver: &IdResolver,
    locator: Option<&str>,
) -> Result<(String, WidgetSnapshot), String> {
    match locator {
        None => Ok(("root widget".to_string(), caps.widget_tree_snapshot().await)),
        Some(id) => {
            let id_uri = EntityUri::parse(id)
                .map_err(|e| format!("[widget] locator {id:?} is not a valid EntityUri: {e}"))?;
            let resolved = resolve_via(resolver, &id_uri);
            let snap = caps.widget_tree_for(&resolved).await.ok_or_else(|| {
                format!("[widget] block {id:?} (resolved {resolved:?}) did not render (no tree)")
            })?;
            Ok((format!("block {id:?}"), snap))
        }
    }
}

async fn widget_haystack(
    caps: &CapMap,
    resolver: &IdResolver,
    locator: Option<&str>,
) -> Result<(String, String), String> {
    let (scope, snap) = widget_scope(caps, resolver, locator).await?;
    Ok((scope, snapshot_text(&snap)))
}

/// Rendered-failure oracle: no `ViewKind::Error` node in the scope. Reads the
/// SAME translated tree every other widget assertion reads, so it sees the
/// per-block live trees `inv-viewmodel-no-error-widgets` cannot (that
/// invariant's cap walks only from `root_layout_block_uri()`).
async fn no_error_widget_caps(
    caps: &CapMap,
    resolver: &IdResolver,
    locator: Option<&str>,
) -> Result<(), String> {
    let (scope, snap) = widget_scope(caps, resolver, locator).await?;
    let errors: Vec<String> = snap
        .walk()
        .filter(|n| n.kind == "error")
        .map(|n| {
            n.props
                .get("message")
                .cloned()
                .unwrap_or_else(|| "<error widget with no message prop>".to_string())
        })
        .collect();
    if errors.is_empty() {
        return Ok(());
    }
    Err(format!(
        "[no-error-widget] {scope} rendered {} error widget(s): {errors:?}",
        errors.len()
    ))
}

/// Negative widget oracle: the rendered text must NOT contain `text`. The
/// failure message quotes the surrounding line so a hit is diagnosable without
/// dumping the whole tree.
async fn widget_omits_caps(
    caps: &CapMap,
    resolver: &IdResolver,
    locator: Option<&str>,
    text: &str,
) -> Result<(), String> {
    let (scope, haystack) = widget_haystack(caps, resolver, locator).await?;
    match haystack.lines().find(|line| line.contains(text)) {
        None => Ok(()),
        Some(hit) => Err(format!(
            "[widget-omits] expected {scope} NOT to contain {text:?}, but it rendered {hit:?}"
        )),
    }
}

/// Slash-menu oracle. Reads `SutEditorMirrorRead::editor_slash_menu_labels` —
/// the labels the operation registry advertises for the block whose editor has
/// the menu open, resolved through the same `CommandProvider` call Enter
/// selects from. A closed menu is a HARD failure either way: an assertion about
/// what a menu does or does not offer is meaningless when no menu is open.
async fn slash_menu_offers_caps(
    caps: &CapMap,
    resolver: &IdResolver,
    block_id: &str,
    label: &str,
    expected: bool,
) -> Result<(), String> {
    let id_uri = EntityUri::parse(block_id)
        .map_err(|e| format!("[slash-menu] {block_id:?} is not a valid EntityUri: {e}"))?;
    let resolved = resolve_via(resolver, &id_uri);
    let labels = caps
        .editor_slash_menu_labels(&resolved)
        .map_err(|e| format!("[slash-menu] labels unreadable for {block_id:?}: {e}"))?
        .ok_or_else(|| {
            format!("[slash-menu] no slash menu is open on block {block_id:?} (type \"/\" first)")
        })?;
    if labels.iter().any(|l| l == label) == expected {
        return Ok(());
    }
    let verb = if expected { "to offer" } else { "NOT to offer" };
    Err(format!(
        "[slash-menu] expected block {block_id:?}'s menu {verb} {label:?}, but it offers {labels:?}"
    ))
}

/// Parentage oracle. Reads the SAME write-side store snapshot the composed
/// catalog's parentage invariants read (`SutBackend::block_raw_snapshot`, the
/// source `inv-no-parent-cycles` and `inv-undo-redo-reference-heal` compare
/// `parent_id` from), so a fixture assertion and an invariant can never
/// disagree about what the store says.
async fn parent_is_caps(
    caps: &CapMap,
    resolver: &IdResolver,
    child_id: &str,
    parent_id: &str,
) -> Result<(), String> {
    let child_uri = EntityUri::parse(child_id)
        .map_err(|e| format!("[parent-is] block id {child_id:?} is not a valid EntityUri: {e}"))?;
    let parent_uri = EntityUri::parse(parent_id)
        .map_err(|e| format!("[parent-is] block id {parent_id:?} is not a valid EntityUri: {e}"))?;
    let child = resolve_via(resolver, &child_uri);
    let parent = resolve_via(resolver, &parent_uri);

    let blocks = caps.block_raw_snapshot().await;
    let known = || {
        let mut ids: Vec<&str> = blocks.iter().map(|b| b.id.as_str()).collect();
        ids.sort_unstable();
        ids.join(", ")
    };
    let Some(block) = blocks.iter().find(|b| b.id == child) else {
        return Err(format!(
            "[parent-is] block {child_id:?} (resolved {child:?}) does not exist in the SUT store \
             — known block ids: [{}]",
            known()
        ));
    };
    // An unknown PARENT id must fail by name too: without this it would only
    // show up as an inequality, hiding a typo behind a plausible-looking diff.
    if !blocks.iter().any(|b| b.id == parent) {
        return Err(format!(
            "[parent-is] parent {parent_id:?} (resolved {parent:?}) does not exist in the SUT \
             store — block {child_id:?} has parent {:?}; known block ids: [{}]",
            block.parent_id,
            known()
        ));
    }
    if block.parent_id == parent {
        return Ok(());
    }
    Err(format!(
        "[parent-is] expected block {child_id:?} (resolved {child:?}) to be a child of \
         {parent_id:?} (resolved {parent:?}), but its store parent is {:?}",
        block.parent_id
    ))
}

/// Sibling-order oracle. Reads `SutSqlProjection::sorted_children` — the
/// `sort_key`-ordered projection `inv-live-children-match-ref` compares against
/// the reference model's `RefBlockTree::sorted_children`, so a fixture ordering
/// assertion and the ordering invariant can never disagree.
async fn sorted_children_of(
    caps: &CapMap,
    resolver: &IdResolver,
    label: &str,
    parent_id: &str,
) -> Result<(EntityUri, Vec<EntityUri>), String> {
    let parent_uri = EntityUri::parse(parent_id)
        .map_err(|e| format!("[{label}] block id {parent_id:?} is not a valid EntityUri: {e}"))?;
    let parent = resolve_via(resolver, &parent_uri);
    let children = caps.sorted_children(&parent).await;
    if children.is_empty() {
        return Err(format!(
            "[{label}] parent {parent_id:?} (resolved {parent:?}) has no children in the SQL \
             projection — nothing to order"
        ));
    }
    Ok((parent, children))
}

fn position_of(children: &[EntityUri], needle: &EntityUri) -> Option<usize> {
    children.iter().position(|c| c == needle)
}

fn render_order(children: &[EntityUri]) -> String {
    children
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}:{c}", i + 1))
        .collect::<Vec<_>>()
        .join(", ")
}

async fn child_index_caps(
    caps: &CapMap,
    resolver: &IdResolver,
    block_id: &str,
    index: usize,
    parent_id: &str,
) -> Result<(), String> {
    if index == 0 {
        return Err(format!(
            "[child-index] `child 0` is not a position: the ordinal is 1-based, so the first child \
             is `child 1` (step named block {block_id:?})"
        ));
    }
    let (_, children) = sorted_children_of(caps, resolver, "child-index", parent_id).await?;
    let block_uri = EntityUri::parse(block_id).map_err(|e| {
        format!("[child-index] block id {block_id:?} is not a valid EntityUri: {e}")
    })?;
    let block = resolve_via(resolver, &block_uri);

    match position_of(&children, &block) {
        Some(pos) if pos + 1 == index => Ok(()),
        Some(pos) => Err(format!(
            "[child-index] expected block {block_id:?} (resolved {block:?}) to be child {index} of \
             {parent_id:?}, but it is child {} — order is [{}]",
            pos + 1,
            render_order(&children)
        )),
        None => Err(format!(
            "[child-index] block {block_id:?} (resolved {block:?}) is not a child of \
             {parent_id:?} — order is [{}]",
            render_order(&children)
        )),
    }
}

async fn comes_after_caps(
    caps: &CapMap,
    resolver: &IdResolver,
    block_id: &str,
    other_id: &str,
) -> Result<(), String> {
    let block_uri = EntityUri::parse(block_id).map_err(|e| {
        format!("[comes-after] block id {block_id:?} is not a valid EntityUri: {e}")
    })?;
    let other_uri = EntityUri::parse(other_id).map_err(|e| {
        format!("[comes-after] block id {other_id:?} is not a valid EntityUri: {e}")
    })?;
    let block = resolve_via(resolver, &block_uri);
    let other = resolve_via(resolver, &other_uri);

    // The common parent comes from the write-side store (the same snapshot
    // `parent_is_caps` reads); comparing positions under different parents
    // would compare incomparable sort keys.
    let blocks = caps.block_raw_snapshot().await;
    let parent_of = |id: &EntityUri, label: &str, raw: &str| -> Result<EntityUri, String> {
        blocks
            .iter()
            .find(|b| &b.id == id)
            .map(|b| b.parent_id.clone())
            .ok_or_else(|| {
                format!(
                    "[comes-after] {label} block {raw:?} (resolved {id:?}) does not exist in the \
                     SUT store"
                )
            })
    };
    let block_parent = parent_of(&block, "left", block_id)?;
    let other_parent = parent_of(&other, "right", other_id)?;
    if block_parent != other_parent {
        return Err(format!(
            "[comes-after] {block_id:?} and {other_id:?} are not siblings — parents are \
             {block_parent:?} and {other_parent:?}; sibling order is only defined within one parent"
        ));
    }

    let children = caps.sorted_children(&block_parent).await;
    let (Some(block_pos), Some(other_pos)) = (
        position_of(&children, &block),
        position_of(&children, &other),
    ) else {
        return Err(format!(
            "[comes-after] {block_id:?} and/or {other_id:?} are missing from the SQL projection's \
             children of {block_parent:?} — order is [{}]",
            render_order(&children)
        ));
    };
    if block_pos > other_pos {
        return Ok(());
    }
    Err(format!(
        "[comes-after] expected block {block_id:?} (resolved {block:?}, child {}) to come after \
         {other_id:?} (resolved {other:?}, child {}) — order is [{}]",
        block_pos + 1,
        other_pos + 1,
        render_order(&children)
    ))
}

/// Task-state oracle. Reads `SutSqlProjection::block_task_state`
/// (`json_extract(properties,'$.task_state')` on `block_raw`) — the same read
/// `inv-task-state-storage-coherence` compares against the Loro projection.
async fn task_state_caps(
    caps: &CapMap,
    resolver: &IdResolver,
    block_id: &str,
    expected: Option<&str>,
) -> Result<(), String> {
    let block_uri = EntityUri::parse(block_id)
        .map_err(|e| format!("[task-state] block id {block_id:?} is not a valid EntityUri: {e}"))?;
    let block = resolve_via(resolver, &block_uri);

    // `block_task_state` answers `None` for both "no such block" and "no
    // keyword"; without this the "has no task state" arm would pass vacuously
    // against a typo'd id.
    if !caps.all_block_ids().await.contains(&block) {
        return Err(format!(
            "[task-state] block {block_id:?} (resolved {block:?}) does not exist in the SQL \
             projection"
        ));
    }

    let actual = caps.block_task_state(&block).await;
    match expected {
        Some(want) if actual.as_deref() == Some(want) => Ok(()),
        Some(want) => Err(format!(
            "[task-state] expected block {block_id:?} (resolved {block:?}) to have task state \
             {want:?}, but the store says {actual:?}"
        )),
        None if actual.as_deref().is_none_or(str::is_empty) => Ok(()),
        None => Err(format!(
            "[task-state] expected block {block_id:?} (resolved {block:?}) to have no task state, \
             but the store says {actual:?}"
        )),
    }
}

/// Fold-state oracle. Reads the write-side `block_raw` snapshot, where
/// `collapsed` is a real column — the same store `parent_is_caps` reads.
async fn collapsed_caps(
    caps: &CapMap,
    resolver: &IdResolver,
    block_id: &str,
    expected: bool,
) -> Result<(), String> {
    let block_uri = EntityUri::parse(block_id)
        .map_err(|e| format!("[collapsed] block id {block_id:?} is not a valid EntityUri: {e}"))?;
    let block = resolve_via(resolver, &block_uri);

    let blocks = caps.block_raw_snapshot().await;
    let Some(found) = blocks.iter().find(|b| b.id == block) else {
        return Err(format!(
            "[collapsed] block {block_id:?} (resolved {block:?}) does not exist in the SUT store"
        ));
    };
    if found.collapsed == expected {
        return Ok(());
    }
    Err(format!(
        "[collapsed] expected block {block_id:?} (resolved {block:?}) to be collapsed={expected}, \
         but the store says collapsed={}",
        found.collapsed
    ))
}

/// Link-resolution oracle. Reads the `block_links` junction, which is the only
/// place a resolved reference differs from a dangling one — the renderer draws
/// the mark's label either way, so no widget assertion can tell them apart.
async fn link_resolves_caps(
    caps: &CapMap,
    resolver: &IdResolver,
    block_id: &str,
    target: &str,
    resolved_id: Option<&str>,
) -> Result<(), String> {
    let block_uri = EntityUri::parse(block_id).map_err(|e| {
        format!("[link-resolves] block id {block_id:?} is not a valid EntityUri: {e}")
    })?;
    let source = resolve_via(resolver, &block_uri);

    let links = caps.block_link_targets(&source).await;
    let render_links = || {
        links
            .iter()
            .map(|(t, r)| format!("{t:?}->{r:?}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let Some((_, actual)) = links.iter().find(|(t, _)| t == target) else {
        return Err(format!(
            "[link-resolves] block {block_id:?} (resolved {source:?}) has no link with target \
             {target:?} — its links are [{}]",
            render_links()
        ));
    };

    match resolved_id {
        None if actual.is_none() => Ok(()),
        None => Err(format!(
            "[link-resolves] expected block {block_id:?}'s link {target:?} to dangle, but it \
             resolves to {actual:?}"
        )),
        Some(want) => {
            let want_uri = EntityUri::parse(want).map_err(|e| {
                format!("[link-resolves] block id {want:?} is not a valid EntityUri: {e}")
            })?;
            let expected = resolve_via(resolver, &want_uri);
            match actual {
                Some(got) if got == &expected => Ok(()),
                Some(got) => Err(format!(
                    "[link-resolves] expected block {block_id:?}'s link {target:?} to resolve to \
                     {want:?} (resolved {expected:?}), but it resolves to {got:?}"
                )),
                None => Err(format!(
                    "[link-resolves] expected block {block_id:?}'s link {target:?} to resolve to \
                     {want:?} (resolved {expected:?}), but it DANGLES (resolved_id is NULL)"
                )),
            }
        }
    }
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
