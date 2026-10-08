//! `RefRenderSources` — what each block's own render must be, derived from
//! its source children the way `BlockDomain::render_entity` and
//! `loro_ui_watcher::derive_render_expr` decide it.
//!
//! A block with a query-source child renders its collection, so only blocks
//! without one are predicted. Of those, a block with exactly one render child
//! draws that render, or an error when it does not parse. A block with several
//! render children is left out: prod picks one of them, and the model does not
//! say which.

use std::collections::BTreeMap;

use holon_api::ContentType;
use holon_api::RenderExpr;
use holon_api::SourceLanguage;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::ExpectedBlockRender;
use holon_pbt_core::capabilities::RefRenderSources;

use crate::pbt::reference_state::ReferenceState;

impl RefRenderSources for ReferenceState {
    fn expected_block_renders(&self) -> BTreeMap<EntityUri, ExpectedBlockRender> {
        let mut sources: BTreeMap<&EntityUri, Vec<&Block>> = BTreeMap::new();
        for block in self.domain.block_state.blocks.values() {
            if block.content_type == ContentType::Source {
                sources.entry(&block.parent_id).or_default().push(block);
            }
        }
        sources
            .into_iter()
            .filter(|(_, children)| {
                !children.iter().any(|c| {
                    c.source_language
                        .as_ref()
                        .is_some_and(|l| l.as_query().is_some())
                })
            })
            .filter_map(|(parent, children)| {
                let renders: Vec<&&Block> = children
                    .iter()
                    .filter(|c| matches!(c.source_language, Some(SourceLanguage::Render)))
                    .collect();
                let [render] = renders.as_slice() else {
                    return None;
                };
                let expected = match holon_api::render_dsl::parse_render_dsl(&render.content) {
                    Ok(expr) => ExpectedBlockRender::Draws {
                        widget: root_widget(&expr, parent),
                        texts: text_literals(&expr),
                    },
                    Err(e) => ExpectedBlockRender::ParseError {
                        parse_error: e.to_string(),
                    },
                };
                Some((parent.clone(), expected))
            })
            .collect()
    }
}

fn root_widget(expr: &RenderExpr, block: &EntityUri) -> String {
    match expr {
        RenderExpr::FunctionCall { name, .. } => name.clone(),
        other => panic!(
            "render source of {block} parses to a non-call root {}; the model predicts only call \
             roots",
            other.to_rhai()
        ),
    }
}

/// The literal of every `text("…")` call in `expr`, in pre-order.
fn text_literals(expr: &RenderExpr) -> Vec<String> {
    let mut out = Vec::new();
    if let RenderExpr::FunctionCall { name, args } = expr
        && name == "text"
        && let Some(RenderExpr::Literal {
            value: Value::String(s),
        }) = args.iter().find(|a| a.name.is_none()).map(|a| &a.value)
    {
        out.push(s.clone());
    }
    for child in expr.children() {
        out.extend(text_literals(child));
    }
    out
}
