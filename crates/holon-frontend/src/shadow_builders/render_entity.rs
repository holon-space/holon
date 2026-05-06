use super::prelude::*;
use crate::render_interpreter::shared_render_entity_build;

holon_macros::widget_builder! {
    raw fn render_entity(ba: BA<'_>) -> ViewModel {
        match shared_render_entity_build(&ba) {
            crate::render_interpreter::RenderBlockResult::ProfileWidget { render, operations } => {
                // Clear render-context flags before interpreting the variant's
                // body. Flags (role, view_mode, embed_depth) describe HOW this
                // entity should render; once `pick_active_variant` has
                // selected the variant, the flags have done their job and
                // descendants render as ordinary content. Without this reset
                // a variant whose body recurses (nested render_entity / live_block)
                // would re-evaluate variant conditions with the same flag and
                // dispatch wrong (e.g., role=page_title leaking down).
                let ctx = ba
                    .ctx
                    .without_flags()
                    .with_operations(operations, ba.services);
                let mut vm = (ba.interpret)(&render, &ctx);
                // Attach the entity-level operations (e.g. indent/outdent/move_*
                // / split_block for blocks) onto the produced ViewModel so
                // `bubble_input` from any descendant can find a matching chord
                // on its way up the focus path.
                //
                // Union with the inner widget's operations rather than
                // attach-when-empty: text blocks render as `editable_text`,
                // which carries its own `set_field` op for content. With
                // attach-when-empty, the block-level chord ops were silently
                // dropped — Tab/Shift+Tab/Enter/Alt+Up/Alt+Down on a focused
                // editor never bubbled to a matching handler, so the keystroke
                // was a silent no-op in production. The chord matcher in
                // `try_handle_node` only fires when an op's `key_chord` matches,
                // so adding `set_field` (key_chord = None) alongside
                // `indent` / `outdent` / ... cannot cause cross-firing.
                for op in ctx.operations.iter() {
                    let already_present = vm.operations.iter().any(|existing| {
                        existing.descriptor.entity_name == op.descriptor.entity_name
                            && existing.descriptor.name == op.descriptor.name
                    });
                    if !already_present {
                        vm.operations.push(op.clone());
                    }
                }
                vm
            }
            crate::render_interpreter::RenderBlockResult::Empty => ViewModel::empty(),
            crate::render_interpreter::RenderBlockResult::Error(msg) => {
                ViewModel::error("render_entity", msg)
            }
        }
    }
}
