mod prelude;
pub use drawer::DEFAULT_DRAWER_WIDTH;
pub use drawer::DRAWER_TOGGLE_WIDTH;

use crate::reactive_view_model::ReactiveViewModel;
use crate::render_context::ContainerCapability;

holon_macros::builder_registry!("src/shadow_builders",
    skip: [prelude],
    register: ReactiveViewModel,
    widget_metas
);

use crate::render_interpreter::RenderInterpreter;

/// Build the shadow `RenderInterpreter<ReactiveViewModel>` from the
/// macro-generated builder registry plus the manual `col` builder.
///
/// **Expensive.** Allocates a `HashMap` and ~34 boxed builders. Intended for
/// **one-shot construction** only. The canonical call site is
/// `HolonFrontendModule::configure()`, which registers a single shared instance
/// in DI and hands it to every consumer via `Arc<RenderInterpreter<_>>`.
///
/// Inside the reactive pipeline nothing ever reaches this function — all
/// interpretation flows through `BuilderServices::interpret` /
/// `BuilderServices::interpret_with_source`, which keep the already-built
/// interpreter behind a trait method and make the type invisible to callers.
///
/// The remaining legitimate call sites are:
/// - `HolonFrontendModule::configure()` — the canonical DI registration
/// - holon-app's `HeadlessBuilderServices::new()` /
///   `StubBuilderServices::new()` — test/stub services that bypass DI and build
///   their own instance
/// - PBT test harnesses that instantiate `ReactiveEngine` manually
///
/// If you find yourself reaching for this function from anything that runs
/// more than once, you probably want `BuilderServices::interpret` instead.
/// Register the authoritative widget name list for the render DSL parser.
///
/// Must be called before any render DSL parsing (entity profile resolution,
/// block rendering, etc.). Safe to call multiple times — `OnceLock` ignores
/// subsequent calls.
pub fn register_render_dsl_widget_names() {
    let mut all_names: Vec<&str> = builder_names().to_vec();
    all_names.extend_from_slice(&[
        "ops_of",
        "focus_chain",
        "chain_ops",
        "state_accent",
        "column",
        "section_stack",
    ]);
    holon_api::render_dsl::register_widget_names(&all_names);
}

pub fn build_shadow_interpreter() -> RenderInterpreter<ReactiveViewModel> {
    register_render_dsl_widget_names();
    let mut interp = RenderInterpreter::new();
    register_all(&mut interp);
    interp.set_widget_metas(all_widget_metas());
    interp.register("column", |ba: prelude::BA<'_>| {
        let gap = ba.args.get_f64("gap").unwrap_or(0.0) as f32;
        // A vertical floor: the column never paints shorter than `min_height`
        // px, so a short/empty section (a LogSeq journal day) still occupies a
        // comfortable block. Parsed at the boundary — present-but-non-numeric or
        // negative is a config bug surfaced loud, never coerced to a default.
        let min_height = match ba.args.get_f64_strict("min_height") {
            Ok(Some(v)) if v < 0.0 => {
                return ReactiveViewModel::error(
                    "column",
                    format!("`min_height` must be >= 0, got {v}"),
                );
            }
            Ok(v) => v,
            Err(msg) => return ReactiveViewModel::error("column", msg),
        };
        // A vertical flow column honours `LayoutHint::PinnedToEnd`: it renders
        // such children at its trailing edge while the rest scrolls. The offer
        // goes to EVERY direct child — the column never inspects what a child
        // is — and the interpreter strips it one level down, so a pin declared
        // from inside a nested `row` stays unhonoured and errors loudly.
        let child_ctx = ba.ctx.offering(ContainerCapability::PinToEnd);
        let children: Vec<ReactiveViewModel> = ba
            .args
            .positional_exprs
            .iter()
            .map(|expr| (ba.interpret)(expr, &child_ctx))
            .collect();
        let mut props = std::collections::HashMap::new();
        props.insert("gap".to_string(), holon_api::Value::Float(gap as f64));
        if let Some(mh) = min_height {
            props.insert("min_height".to_string(), holon_api::Value::Float(mh));
        }
        ReactiveViewModel {
            children: children.into_iter().map(std::sync::Arc::new).collect(),
            ..ReactiveViewModel::from_widget("column", props)
        }
    });
    interp.register("section_stack", |ba: prelude::BA<'_>| {
        // Section-stack container (Inc C): a scroll region of variable-height
        // sections, each optionally bearing an in-flow (`pinned:false`) or
        // `sticky` accordion. Same capability discipline as `column`, offering
        // scroll-integrated sections instead of trailing-edge pinning — so a
        // pinned accordion here (or a section-stack accordion elsewhere) errors
        // loudly.
        let gap = ba.args.get_f64("gap").unwrap_or(0.0) as f32;
        let child_ctx = ba.ctx.offering(ContainerCapability::ScrollSections);
        let children: Vec<ReactiveViewModel> = ba
            .args
            .positional_exprs
            .iter()
            .map(|expr| (ba.interpret)(expr, &child_ctx))
            .collect();
        let mut props = std::collections::HashMap::new();
        props.insert("gap".to_string(), holon_api::Value::Float(gap as f64));
        props.insert("section_stack".to_string(), holon_api::Value::Boolean(true));
        ReactiveViewModel {
            children: children.into_iter().map(std::sync::Arc::new).collect(),
            ..ReactiveViewModel::from_widget("section_stack", props)
        }
    });
    // Value functions — registered alongside widgets, disjoint name
    // space (collision-checked by `register_value_fn`).
    crate::value_fns::register_ops_of(&mut interp);
    crate::value_fns::register_focus_chain(&mut interp);
    crate::value_fns::register_chain_ops(&mut interp);
    crate::value_fns::register_state_accent(&mut interp);
    interp
}

// all_widget_metas() is auto-generated by builder_registry! above

/// Dispatch `resolve_props_from_args` for a named builder.
///
/// Returns `Some(props)` when the builder has a macro-generated
/// `resolve_props_from_args` (auto-body and custom-body non-raw widgets).
/// Returns `None` for raw builders that don't have this function.
pub(crate) fn dispatch_resolve_props(
    widget_name: &str,
    ba: &prelude::BA<'_>,
) -> Option<std::collections::HashMap<String, holon_api::Value>> {
    match widget_name {
        "text" => Some(text::resolve_props_from_args(ba)),
        "badge" => Some(badge::resolve_props_from_args(ba)),
        "icon" => Some(icon::resolve_props_from_args(ba)),
        "checkbox" => Some(checkbox::resolve_props_from_args(ba)),
        "source_block" => Some(source_block::resolve_props_from_args(ba)),
        "source_editor" => Some(source_editor::resolve_props_from_args(ba)),
        "editable_text" => Some(editable_text::resolve_props_from_args(ba)),
        "rendered_text" => Some(rendered_text::resolve_props_from_args(ba)),
        _ => None,
    }
}
