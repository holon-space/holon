use futures_signals::signal::SignalExt;

use super::prelude::*;
use crate::reactive_view_model::DropTask;
use crate::reactive_view_model::LazyReactiveSlot;

/// Read the `collapsed` column as a bool. Stored as SQLite `INTEGER 0/1`
/// (see `turso_value_to_value` — bool columns always come back as
/// `Value::Integer` on read, never `Value::Boolean`, which is only used on
/// the write side); absent/NULL defaults to not-collapsed (expanded).
fn row_collapsed(row: &holon_api::widget_spec::DataRow) -> bool {
    row.get("collapsed").and_then(|v| v.as_i64()).unwrap_or(0) != 0
}

holon_macros::widget_builder! {
    fn expand_toggle(header: Expr, content: Expr) -> ViewModel {
        let target_id = ba.ctx.row().get("id")
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string();

        // The lazy gate starts CLOSED by default: `expand_toggle` is a
        // lazy-section widget (claude-history style) whose default is
        // collapsed-until-clicked — a block row's default `collapsed = 0`
        // means "not explicitly folded", NOT "auto-expand this section".
        // (The outline tree's `tree_item` wrapper is the widget whose default
        // seeds from the row — see `wrap_tree_item`.) Document-state collapse
        // still flows both ways here: the GPUI chevron dispatches
        // `set_field(collapsed)` on click, and the subscription below
        // folds/unfolds the gate when the field CHANGES externally.
        //
        // A particular embedding context may seed the gate OPEN via
        // `default_expanded: true` (the Journal-feed knob): the lazy content
        // then materialises eagerly (children loaded) instead of on first
        // click, while the global default stays collapsed+lazy. Fail loud if
        // the arg is present but not a boolean — never silently coerce config.
        let default_expanded = match ba.args.get_bool_strict("default_expanded") {
            Ok(v) => v.unwrap_or(false),
            Err(msg) => return ViewModel::error("expand_toggle", msg),
        };
        // Placement of the toggle affordance. Default (false): a LEADING
        // chevron in the left gutter, always visible. When true: the chevron
        // moves to the RIGHT of the header and is revealed only while the
        // header row is hovered — used for embedded pages that ALSO carry a
        // tree collapse chevron so the two affordances don't sit adjacent in
        // the gutter (double-chevron bug, ruling 2026-07-21). The GPUI builder
        // reads this prop and, when set, drives the `hovered` gate seeded
        // below (same idiom as the `on_hover` widget).
        let hover_reveal_toggle = match ba.args.get_bool_strict("hover_reveal_toggle") {
            Ok(v) => v.unwrap_or(false),
            Err(msg) => return ViewModel::error("expand_toggle", msg),
        };
        let initial_collapsed = row_collapsed(ba.ctx.row());
        // Seed the gate from the engine's view-local expansion store (RATIFIED
        // 2026-07-16, Option B) when the user has driven this toggle; otherwise
        // fall back to the `default_expanded` embedding knob (itself `false`
        // unless the context seeds it open — the Journal-feed case). This is
        // what makes a driven expand survive a fresh `snapshot()` for
        // profile-driven embedded pages, which carry no `collapsed` document
        // field. Precedence: explicit store entry > default_expanded > false.
        let seed_expanded = ba.services.block_expanded_view(&target_id).unwrap_or(default_expanded);
        let seed_target = target_id.clone();
        let expanded = futures_signals::signal::Mutable::new(seed_expanded);

        let header = header
            .cloned()
            .unwrap_or_else(|| holon_api::render_types::RenderExpr::FunctionCall {
                name: "text".to_string(),
                args: vec![],
            });

        let header_vm = (ba.interpret)(&header, ba.ctx);
        let children = vec![Arc::new(header_vm)];

        // Lazy materialisation: capture services + template + ctx so the
        // content is interpreted on first expand instead of at build time.
        // Why this matters: claude-history.yaml wraps live_query inside
        // expand_toggle content; interpreting it while collapsed spawns
        // unnecessary FDW fetches. The cache lives for the VM lifetime, so
        // re-collapse + re-expand is instant. `push_down_lazy_slot` carries
        // the cache forward across structural rebuilds.
        let lazy_slot = content.cloned().map(|template| {
            let services_arc = ba.services.clone_arc();
            let ctx = ba.ctx.clone();
            let thunk: Arc<dyn Fn() -> ViewModel + Send + Sync> =
                Arc::new(move || services_arc.interpret(&template, &ctx));
            LazyReactiveSlot::new(expanded.read_only(), thunk)
        });

        let mut __props = std::collections::HashMap::new();
        __props.insert("target_id".to_string(), Value::String(target_id));
        __props.insert(
            "hover_reveal_toggle".to_string(),
            Value::Boolean(hover_reveal_toggle),
        );

        let data = ba.ctx.data_mutable();
        let mut vm = ViewModel {
            expanded: Some(expanded.clone()),
            // Per-render-slot hover gate (only in trailing-reveal mode) — same
            // Mutable idiom as `on_hover`; carried across rebuilds so a hover
            // survives repaints (see `with_update`/`push_down_children`).
            hovered: hover_reveal_toggle
                .then(|| futures_signals::signal::Mutable::new(false)),
            lazy_slot,
            children,
            operations: ba.ctx.operations.clone(),
            data: data.clone(),
            render_ctx: Some(ba.ctx.clone()),
            ..ViewModel::from_widget("expand_toggle", __props)
        };

        // Live-follow CHANGES to the `collapsed` column: an external
        // `set_field` (another device, undo, MCP) fires CDC through `data`'s
        // shared cell; fold/unfold the gate accordingly. Only edge-triggered
        // (`last` guard) — the initial emission and unrelated row updates
        // must not clobber the widget's collapsed-by-default lazy gate or a
        // local click that hasn't echoed yet. Skipped in sync-only contexts
        // (PBT reference model, snapshot interpretation) — same rationale as
        // `state_toggle`.
        if let Some(runtime) = ba.services.try_runtime_handle() {
            let expanded_handle = expanded.clone();
            let follow_services = ba.services.clone_arc();
            let follow_target = seed_target.clone();
            let mut last = initial_collapsed;
            let task = runtime.spawn(data.signal_cloned().for_each(move |row| {
                let collapsed = row_collapsed(&row);
                if collapsed != last {
                    last = collapsed;
                    expanded_handle.set(!collapsed);
                    // Only the FOLD edge is durable. `collapsed` belongs to the
                    // outline row's own chevron, so propagating the UNFOLD edge
                    // would let unfolding the outline open — and eagerly load —
                    // a nested page nobody clicked.
                    if collapsed {
                        follow_services.set_block_expanded_view(&follow_target, false);
                    }
                }
                async {}
            }));
            vm.subscriptions.push(DropTask::new(task));
        }

        vm
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use holon_api::Value;
    use holon_api::widget_spec::DataRow;

    use crate::reactive::BuilderServices;
    use crate::reactive::StubBuilderServices;

    fn interpret_toggle(src: &str) -> crate::reactive_view_model::ReactiveViewModel {
        crate::shadow_builders::register_render_dsl_widget_names();
        let expr = holon_api::render_dsl::parse_render_dsl(src).expect("render DSL parses");
        let mut row = DataRow::new();
        row.insert("id".into(), Value::String("block:page-a".into()));
        row.insert("content".into(), Value::String("A Page".into()));
        let ctx = crate::RenderContext::default().with_row(Arc::new(row));
        let services = StubBuilderServices::new();
        let vm = services.interpret(&expr, &ctx);
        assert_eq!(vm.widget_name().as_deref(), Some("expand_toggle"));
        vm
    }

    /// The `hover_reveal_toggle` knob plumbs from the render DSL through to the
    /// VM: the prop is set AND a `hovered` gate is seeded (the GPUI builder
    /// keys its trailing hover-reveal chevron off both). Guards the
    /// double-chevron fix (ruling 2026-07-21) at the model layer.
    #[test]
    fn hover_reveal_toggle_plumbs_prop_and_seeds_hover_gate() {
        let vm = interpret_toggle(
            r#"expand_toggle(#{hover_reveal_toggle: true, header: text("A Page"), content: text("body")})"#,
        );
        assert_eq!(vm.prop_bool("hover_reveal_toggle"), Some(true));
        assert!(
            vm.hovered.is_some(),
            "trailing hover-reveal mode must seed a `hovered` Mutable"
        );
    }

    /// `default_expanded: true` seeds the gate open, so the snapshot
    /// materialises the lazy content child instead of rendering header-only.
    /// Pins the builder's documented contract (BugFunnel 2026-08-02).
    #[test]
    fn default_expanded_materialises_content_in_snapshot() {
        let vm = interpret_toggle(
            r#"expand_toggle(#{default_expanded: true, header: text("A Page"), content: text("HELLO-STATIC")})"#,
        );
        let snap = vm.snapshot();
        let rendered = snap.pretty_print(0);
        assert!(
            rendered.contains("HELLO-STATIC"),
            "default_expanded toggle must materialise its content; got:\n{rendered}"
        );
    }

    /// The `content:` arg reaches the lazy slot and materialises once the
    /// gate is open. Guards the class of bug where a widget's template arg is
    /// classified as a scalar and silently never renders.
    #[test]
    fn content_template_materialises_when_expanded() {
        let vm = interpret_toggle(
            r#"expand_toggle(#{default_expanded: true, header: text("A Page"), content: text("body")})"#,
        );
        let slot = vm
            .lazy_slot
            .as_ref()
            .expect("content: must build a lazy slot");
        let content = slot
            .materialize_if_gated()
            .expect("an expanded toggle must materialise its content");
        assert_eq!(content.widget_name().as_deref(), Some("text"));
    }

    /// Default expand_toggle (no knob) keeps the leading-chevron shape: the
    /// prop is false and NO hover gate is allocated.
    #[test]
    fn default_expand_toggle_has_no_hover_gate() {
        let vm =
            interpret_toggle(r#"expand_toggle(#{header: text("A Page"), content: text("body")})"#);
        assert_eq!(vm.prop_bool("hover_reveal_toggle"), Some(false));
        assert!(
            vm.hovered.is_none(),
            "leading-chevron mode must not allocate a `hovered` gate"
        );
    }
}
