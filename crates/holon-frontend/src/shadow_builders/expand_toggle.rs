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
    raw fn expand_toggle(ba: BA<'_>) -> ViewModel {
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
        let initial_collapsed = row_collapsed(ba.ctx.row());
        // Seed the gate from the engine's view-local expansion store (RATIFIED
        // 2026-07-16, Option B) when the user has driven this toggle; otherwise
        // fall back to the `default_expanded` embedding knob (itself `false`
        // unless the context seeds it open — the Journal-feed case). This is
        // what makes a driven expand survive a fresh `snapshot()` for
        // profile-driven embedded pages, which carry no `collapsed` document
        // field. Precedence: explicit store entry > default_expanded > false.
        let seed_expanded = ba.services.block_expanded_view(&target_id).unwrap_or(default_expanded);
        let expanded = futures_signals::signal::Mutable::new(seed_expanded);

        let header = ba.args.get_template("header")
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
        let lazy_slot = ba.args.get_template("content").cloned().map(|template| {
            let services_arc = ba.services.clone_arc();
            let ctx = ba.ctx.clone();
            let thunk: Arc<dyn Fn() -> ViewModel + Send + Sync> =
                Arc::new(move || services_arc.interpret(&template, &ctx));
            LazyReactiveSlot::new(expanded.read_only(), thunk)
        });

        let mut __props = std::collections::HashMap::new();
        __props.insert("target_id".to_string(), Value::String(target_id));

        let data = ba.ctx.data_mutable();
        let mut vm = ViewModel {
            expanded: Some(expanded.clone()),
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
            let mut last = initial_collapsed;
            let task = runtime.spawn(data.signal_cloned().for_each(move |row| {
                let collapsed = row_collapsed(&row);
                if collapsed != last {
                    last = collapsed;
                    expanded_handle.set(!collapsed);
                }
                async {}
            }));
            vm.subscriptions.push(DropTask::new(task));
        }

        vm
    }
}
