use super::prelude::*;

// `block_id` names the entity a literal-label badge marks. A badge built from
// `badge(col("status"))` binds its row through the label; one whose label is a
// literal — a disclosure the profile writes, not the data — has no binding at
// all, and a mark nobody can attribute to a block is not a mark.
holon_macros::widget_builder! {
    // ALLOW(raw_block_id_widget_param): the body below classifies the arg, and
    // `badge` is off `dispatch_resolve_props` so the generated verbatim props
    // function is never the one that runs
    fn badge(label: String, block_id: Option<String>) {
        // `#{block_id: col("id")}` reads the vault's own SQL, so a value that
        // forms no URI is content, not a programming error. Classify it here
        // (D142.a): refuse the unusable one, and carry the entity one in its
        // SCHEMED form — `entity_id()` parses the prop strictly, so a bare
        // `abc` left verbatim would unwind there just as `my task` did.
        let block_id = match block_id.as_deref().map(holon_api::row_id_of_str) {
            Some(holon_api::RowId::Unusable(refusal)) => return ViewModel::refused_row(&refusal),
            Some(holon_api::RowId::Entity(uri)) => Some(uri.to_string()),
            Some(holon_api::RowId::Absent) | None => None,
        };

        // Exactly the map the macro's `resolve_props_from_args` builds for
        // these two params, with `block_id` in its classified form.
        let mut __props = std::collections::HashMap::new();
        __props.insert("label".to_string(), Value::String(label));
        if let Some(id) = block_id {
            __props.insert("block_id".to_string(), Value::String(id));
        }
        ViewModel::from_widget("badge", __props)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use holon_api::Value;
    use holon_api::widget_spec::DataRow;

    use crate::reactive::BuilderServices;
    use crate::reactive::StubBuilderServices;
    use crate::reactive_view_model::ReactiveViewModel;

    /// Interpret `src` against one row whose `id` column is `id`, the way a
    /// profile's own render expression reaches this builder.
    fn interpret_with_row(src: &str, id: &str) -> ReactiveViewModel {
        crate::shadow_builders::register_render_dsl_widget_names();
        let mut row = DataRow::new();
        row.insert("id".to_string(), Value::String(id.to_string()));
        row.insert("content".to_string(), Value::String("hello".to_string()));
        let ctx = crate::RenderContext::default().with_row(Arc::new(row));
        let expr = holon_api::render_dsl::parse_render_dsl(src).expect("render DSL parses");
        StubBuilderServices::new().interpret(&expr, &ctx)
    }

    const MARKS_THE_ROW: &str = r#"badge("Page", #{block_id: col("id")})"#;

    /// The `id` column is the vault's own SQL. One that forms no URI used to
    /// ride into the `block_id` prop verbatim and unwind in `entity_id()`,
    /// which every reader of a marked badge calls.
    #[test]
    fn a_block_id_that_forms_no_uri_is_refused_not_stamped_into_the_prop() {
        let vm = interpret_with_row(MARKS_THE_ROW, "my task");
        assert_eq!(vm.widget_name().as_deref(), Some("error"));
        let message = vm
            .prop_str("message")
            .expect("the refusal carries a message");
        assert!(
            message.contains("my task"),
            "the refusal must name the id the vault wrote, got {message:?}"
        );
        assert!(vm.entity_id().is_none(), "a refused badge names no entity");
    }

    /// The subtler half: a BARE id forms a URI once schemed, so it is usable —
    /// but `entity_id()` parses the prop strictly, so the prop must carry the
    /// schemed form, not the bare text.
    #[test]
    fn a_bare_block_id_is_carried_in_its_schemed_form() {
        let vm = interpret_with_row(MARKS_THE_ROW, "abc");
        assert_eq!(vm.widget_name().as_deref(), Some("badge"));
        assert_eq!(vm.prop_str("block_id").as_deref(), Some("block:abc"));
        assert_eq!(
            vm.entity_id().map(|u| u.to_string()),
            Some("block:abc".to_string())
        );
    }

    /// A badge that marks nothing keeps no `block_id` prop at all, so
    /// `entity_id()` falls through to the row binding instead of parsing "".
    #[test]
    fn a_badge_with_no_block_id_carries_no_prop() {
        let vm = interpret_with_row(r#"badge(col("content"))"#, "block:r1");
        assert_eq!(vm.widget_name().as_deref(), Some("badge"));
        assert_eq!(vm.prop_str("block_id"), None);
    }

    /// An EMPTY `id` column is `RowId::Absent`, not a phantom `block:` — the
    /// prop is dropped rather than carried as text `entity_id()` would parse.
    #[test]
    fn an_empty_block_id_carries_no_prop() {
        let vm = interpret_with_row(MARKS_THE_ROW, "");
        assert_eq!(vm.widget_name().as_deref(), Some("badge"));
        assert_eq!(vm.prop_str("block_id"), None);
    }
}
