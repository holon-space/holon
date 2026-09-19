use super::prelude::*;

holon_macros::widget_builder! {
    raw fn live_block(ba: BA<'_>) -> ViewModel {
        match crate::render_interpreter::live_block_target(&ba) {
            Ok(block_id) => ViewModel::live_block(block_id),
            Err(msg) => ViewModel::error("live_block", msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::reactive::BuilderServices;
    use crate::reactive::StubBuilderServices;

    fn interpret(src: &str) -> crate::reactive_view_model::ReactiveViewModel {
        crate::shadow_builders::register_render_dsl_widget_names();
        let expr = holon_api::render_dsl::parse_render_dsl(src).expect("render DSL parses");
        StubBuilderServices::new().interpret(&expr, &crate::RenderContext::default())
    }

    /// The id is author-written render DSL. One that forms no URI used to
    /// unwind inside `EntityUri`, taking the whole window with it; it now
    /// names itself in an error node the author can read.
    #[test]
    fn an_id_that_forms_no_uri_becomes_an_error_node_naming_it() {
        let vm = interpret(r#"live_block("my task")"#);
        assert_eq!(vm.widget_name().as_deref(), Some("error"));
        let message = vm
            .prop_str("message")
            .expect("the error node carries a message");
        assert!(
            message.contains("my task"),
            "the error must name the id the author wrote, got {message:?}"
        );
    }

    #[test]
    fn a_schemed_id_still_builds_the_live_block() {
        let vm = interpret(r#"live_block("block:page-a")"#);
        assert_eq!(vm.widget_name().as_deref(), Some("live_block"));
        assert_eq!(vm.prop_str("block_id").as_deref(), Some("block:page-a"));
    }
}
