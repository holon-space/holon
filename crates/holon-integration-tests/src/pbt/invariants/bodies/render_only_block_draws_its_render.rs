//! `inv-render-only-block-draws-its-render` — a block whose only source child
//! is a render draws that render (its own `watch_ui`).
//!
//! @pbt oracle correspondence
//! @pbt covers render-only-panel-renders-its-render — a block whose only
//!   source child is a render draws that render, not a bare entity leaf
//! @pbt slips-if-removed a panel authored as a render alone shows its heading
//!   as a plain block instead of the panel
//!
//! The reference predicts the render from the block's source children
//! (`RefRenderSources`); the SUT side is `SutRenderer::widget_tree_for`, the
//! headless interpretation of the block's own watch.

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::ExpectedBlockRender;
use holon_pbt_core::capabilities::RefRenderSources;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::WidgetSnapshot;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvRenderOnlyBlockDrawsItsRender;

impl InvRenderOnlyBlockDrawsItsRender {
    pub const ID: InvariantId = InvariantId("inv-render-only-block-draws-its-render");
}

async fn block_tree<S: SutRenderer>(sut: &S, id: &EntityUri) -> Result<WidgetSnapshot, String> {
    sut.widget_tree_for(id)
        .await
        .ok_or_else(|| format!("{id}: its own render never resolved"))
}

/// `kind(child, …)` of the tree, for failure messages.
fn outline(tree: &WidgetSnapshot) -> String {
    if tree.children.is_empty() {
        tree.kind.clone()
    } else {
        let children: Vec<String> = tree.children.iter().map(outline).collect();
        format!("{}({})", tree.kind, children.join(", "))
    }
}

fn draws_mismatch(
    id: &EntityUri,
    tree: &WidgetSnapshot,
    widget: &str,
    texts: &[String],
) -> Option<String> {
    if tree.kind != widget {
        return Some(format!(
            "{id}: root widget is `{}`, the render source draws `{widget}` (tree: {})",
            tree.kind,
            outline(tree)
        ));
    }
    let shown: Vec<&str> = tree
        .walk()
        .filter(|n| n.kind == "text")
        .filter_map(|n| n.props.get("content").map(String::as_str))
        .collect();
    texts
        .iter()
        .find(|t| !shown.contains(&t.as_str()))
        .map(|t| format!("{id}: the render's text {t:?} is not drawn; texts drawn: {shown:?}"))
}

fn error_mismatch(id: &EntityUri, tree: &WidgetSnapshot, parse_error: &str) -> Option<String> {
    if tree.kind != "error" {
        return Some(format!(
            "{id}: a render source that does not parse must draw an `error` widget, got `{}` \
             (tree: {})",
            tree.kind,
            outline(tree)
        ));
    }
    let message = tree.props.get("message").map(String::as_str).unwrap_or("");
    (!message.contains(id.as_str()) || !message.contains(parse_error)).then(|| {
        format!(
            "{id}: the error must name the block and the parse error {parse_error:?}; got \
             {message:?}"
        )
    })
}

/// Judge every block the model expects to draw (`parse_errors == false`) or to
/// show its parse error (`parse_errors == true`).
pub(crate) async fn check_kind<R: RefRenderSources, S: SutRenderer>(
    ref_: &R,
    sut: &S,
    label: &str,
    parse_errors: bool,
) -> InvariantResult {
    let expected: Vec<(EntityUri, ExpectedBlockRender)> = ref_
        .expected_block_renders()
        .into_iter()
        .filter(|(_, e)| matches!(e, ExpectedBlockRender::ParseError { .. }) == parse_errors)
        .collect();
    if expected.is_empty() {
        return InvariantResult::Skipped(format!("[{label}] no such block in the model"));
    }
    let mut failures = Vec::new();
    for (id, expectation) in &expected {
        let tree = match block_tree(sut, id).await {
            Ok(tree) => tree,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        let mismatch = match expectation {
            ExpectedBlockRender::Draws { widget, texts } => {
                draws_mismatch(id, &tree, widget, texts)
            }
            ExpectedBlockRender::ParseError { parse_error } => {
                error_mismatch(id, &tree, parse_error)
            }
        };
        failures.extend(mismatch);
    }
    if failures.is_empty() {
        InvariantResult::Ok
    } else {
        InvariantResult::Fail(format!("[{label}] {}", failures.join("; ")))
    }
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvRenderOnlyBlockDrawsItsRender
where
    R: RefRenderSources,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        check_kind(ref_, sut, Self::ID.0, false).await
    }
}
