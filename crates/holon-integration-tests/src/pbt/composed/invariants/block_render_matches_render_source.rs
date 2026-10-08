//! A block's own render follows its render source child. `Needs SutRenderer +
//! RefRenderSources`: every slice with a renderer selects both, in both
//! render arms (Turso `render_entity`, Loro `derive_render_expr`).

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::RefRenderSources;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Layer;
use holon_pbt_core::composition::Needs;

use crate::pbt::invariants::bodies::render_only_block_draws_its_render::InvRenderOnlyBlockDrawsItsRender;
use crate::pbt::invariants::bodies::unparseable_render_is_error_node::InvUnparseableRenderIsErrorNode;

fn needs() -> Needs {
    Needs {
        sut_present: vec![CapId::of::<dyn SutRenderer>()],
        sut_absent: Vec::new(),
        ref_present: vec![CapId::of::<dyn RefRenderSources>()],
    }
}

pub fn wire_render_only() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvRenderOnlyBlockDrawsItsRender,
        RunMode::Strict,
        needs(),
        Attribution::at(Layer::ViewModel, file!()),
    ))
}

pub fn wire_unparseable() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvUnparseableRenderIsErrorNode,
        RunMode::Strict,
        needs(),
        Attribution::at(Layer::ViewModel, file!()),
    ))
}
