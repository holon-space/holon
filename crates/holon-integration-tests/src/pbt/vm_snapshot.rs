//! Frontend-agnostic ViewModel→WidgetSnapshot translation for PBT invariants.
//!
//! Extracted from the deleted `sut_capabilities.rs` (E2ESut monolith); consumed
//! by the composed component hosts (`frontend_slice`, `window_slice`).

/// Frontend-agnostic ViewModel→WidgetSnapshot translator.
///
/// Encoding contract (see `WidgetSnapshot` rustdoc in pbt-core):
/// - `kind` = `vm.widget_name()` or `"unknown"` for un-tagged kinds.
/// - `entity_id` = `vm.row_id()` if present.
/// - `props` = kind-specific scalar fields canonicalised to strings
///   (StateToggle.field/current/label/states, EditableText.field/content,
///   etc.).
/// - `operations` = canonical form
///   `<op_name>:<affected_fields_csv>:<param_names_csv>`, one per
///   `OperationWiring`. Invariants match on this form — e.g. by op-name prefix,
///   or (for "an op that changes field X") by the affected-fields segment
///   (`set_state:task_state:…` / `cycle_task_state:task_state:…`).
pub(crate) fn view_model_to_snapshot(
    vm: &holon_frontend::view_model::ViewModel,
) -> holon_pbt_core::capabilities::WidgetSnapshot {
    use std::collections::BTreeMap;

    use holon_frontend::view_model::ViewKind;

    // `widget_name()` returns `None` for BOTH `Empty` and `Loading`, but they
    // mean opposite things to a snapshot consumer: `Loading` is a TRANSIENT
    // placeholder (a reactive watch hasn't delivered yet — worth re-sampling),
    // `Empty` is a PERMANENT structural slot (e.g. a tree item's vacant
    // region). Conflating them as "unknown" made `widget_tree_snapshot`'s
    // pending detector treat every tree as never-resolved, costing the full
    // cautious resample window on every check (measured: the keystone's
    // dominant wall-time cost). Name them explicitly.
    let kind = match &vm.kind {
        ViewKind::Loading => "loading".to_string(),
        ViewKind::Empty => "empty".to_string(),
        _ => vm.widget_name().unwrap_or("unknown").to_string(),
    };
    // For LiveBlock, the referenced block id lives in `kind.block_id`;
    // `vm.row_id()` returns None. Mirror `ViewModel::collect_ids_recursive`
    // semantics by surfacing block_id as entity_id so cross-slice
    // invariants can walk references without knowing about LiveBlock.
    let entity_id = match &vm.kind {
        ViewKind::LiveBlock { block_id, .. } => Some(block_id.clone()),
        _ => vm.row_id(),
    };

    let mut props: BTreeMap<String, String> = BTreeMap::new();
    match &vm.kind {
        ViewKind::StateToggle {
            field,
            current,
            label,
            states,
        } => {
            props.insert("field".into(), field.clone());
            props.insert("current".into(), current.clone());
            props.insert("label".into(), label.clone());
            props.insert("states".into(), states.clone());
        }
        ViewKind::EditableText { content, field } => {
            props.insert("field".into(), field.clone());
            props.insert("content".into(), content.clone());
            // Encode trigger count so `inv-viewmodel-editable-text-triggers`
            // can assert non-empty triggers for editors with bound operations
            // without exposing the InputTrigger type to the cross-slice IR.
            props.insert("trigger_count".into(), vm.triggers.len().to_string());
        }
        ViewKind::RenderedText { content, field } => {
            props.insert("field".into(), field.clone());
            props.insert("content".into(), content.clone());
        }
        ViewKind::Text { content, .. } => {
            props.insert("content".into(), content.clone());
        }
        ViewKind::Badge { label } => {
            props.insert("label".into(), label.clone());
        }
        _ => {}
    }

    // Placed rows are identified in PBT snapshots by props["occurrence"] =
    // key_suffix hex; invariants recompute OccurrenceId::for_placement(canonical,
    // anchor) to match. Canonical nodes stamp nothing so every existing snapshot
    // stays byte-identical until advice weaving lands.
    if let holon_api::Occurrence::Placed(_) = vm.occurrence {
        props.insert("occurrence".into(), vm.occurrence.key_suffix());
    }

    let operations: Vec<String> = vm
        .operations
        .iter()
        .map(|ow| {
            let fields_csv = ow.descriptor.affected_fields.join(",");
            let params_csv = ow
                .descriptor
                .required_params
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(",");
            format!("{}:{}:{}", ow.descriptor.name, fields_csv, params_csv)
        })
        .collect();

    let children = vm.children().iter().map(view_model_to_snapshot).collect();

    holon_pbt_core::capabilities::WidgetSnapshot {
        kind,
        entity_id,
        props,
        operations,
        children,
    }
}

#[cfg(test)]
mod tests {
    use holon_api::EntityUri;
    use holon_api::Occurrence;
    use holon_api::OccurrenceId;
    use holon_frontend::view_model::ViewKind;
    use holon_frontend::view_model::ViewModel;

    use super::view_model_to_snapshot;

    fn text_node(occurrence: Occurrence) -> ViewModel {
        ViewModel {
            kind: ViewKind::Text {
                content: "hi".into(),
                bold: false,
                size: 14.0,
                color: None,
            },
            occurrence,
            ..Default::default()
        }
    }

    #[test]
    fn placed_occurrence_is_stamped_into_props() {
        let canonical = EntityUri::block("row");
        let anchor = EntityUri::block("anchor");
        let occ = OccurrenceId::for_placement(&canonical, &anchor);
        let snap = view_model_to_snapshot(&text_node(Occurrence::Placed(occ.clone())));
        assert_eq!(snap.props.get("occurrence"), Some(&occ.key_suffix()));
    }

    #[test]
    fn canonical_occurrence_stamps_no_props_entry() {
        let snap = view_model_to_snapshot(&text_node(Occurrence::Canonical));
        assert!(!snap.props.contains_key("occurrence"));
    }
}
