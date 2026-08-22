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
            appearance,
            binding,
        } => {
            props.insert("field".into(), field.clone());
            props.insert("current".into(), current.clone());
            props.insert("label".into(), label.clone());
            props.insert("states".into(), states.clone());
            props.insert("binding".into(), binding.as_str().to_string());
            // The headless tier cannot see the painted control, but it CAN see
            // which one was asked for — the half of the switch appearance a
            // snapshot oracle is able to hold.
            props.insert("appearance".into(), appearance.as_str().to_string());
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
        ViewKind::ExpandToggle {
            target_id,
            expanded,
            ..
        } => {
            props.insert("target_id".into(), target_id.clone());
            props.insert("expanded".into(), expanded.to_string());
        }
        ViewKind::Drawer {
            block_id,
            mode,
            open,
            width,
            ..
        } => {
            props.insert("block_id".into(), block_id.clone());
            props.insert("mode".into(), mode.as_str().into());
            props.insert("open".into(), open.to_string());
            props.insert("width".into(), width.to_string());
        }

        // Every remaining kind, spelled out. The match is EXHAUSTIVE on
        // purpose: a new `ViewKind` must decide what a snapshot consumer can
        // see of it, and the old `_ => {}` silently answered "nothing" — which
        // is how a generic widget's title and an error message became
        // unobservable to every fixture assertion (BugFunnel
        // 2026-08-22-backlinks-section-not-observable-headless).
        ViewKind::Error { message } => {
            props.insert("message".into(), message.clone());
        }
        ViewKind::Icon { name, size } => {
            props.insert("name".into(), name.clone());
            props.insert("size".into(), size.to_string());
        }
        ViewKind::Checkbox { checked } => {
            props.insert("checked".into(), checked.to_string());
        }
        ViewKind::Spacer {
            width,
            height,
            color,
        } => {
            props.insert("width".into(), width.to_string());
            props.insert("height".into(), height.to_string());
            if let Some(color) = color {
                props.insert("color".into(), color.clone());
            }
        }
        ViewKind::Image {
            path,
            alt,
            width,
            height,
        } => {
            props.insert("path".into(), path.clone());
            props.insert("alt".into(), alt.clone());
            if let Some(width) = width {
                props.insert("width".into(), width.to_string());
            }
            if let Some(height) = height {
                props.insert("height".into(), height.to_string());
            }
        }
        ViewKind::Section { title, .. } => {
            props.insert("title".into(), title.clone());
        }
        ViewKind::Collapsible { header, icon, .. } => {
            props.insert("header".into(), header.clone());
            props.insert("icon".into(), icon.clone());
        }
        ViewKind::Card { accent, .. } => {
            props.insert("accent".into(), accent.clone());
        }
        ViewKind::ChatBubble { sender, time, .. } => {
            props.insert("sender".into(), sender.clone());
            props.insert("time".into(), time.clone());
        }
        ViewKind::Board { lane_field, .. } => {
            props.insert("lane_field".into(), lane_field.clone());
        }
        ViewKind::Row { gap, .. } | ViewKind::List { gap, .. } | ViewKind::Columns { gap, .. } => {
            props.insert("gap".into(), gap.to_string());
        }
        ViewKind::Column { gap, .. } => {
            props.insert("gap".into(), gap.to_string());
        }
        ViewKind::SourceBlock {
            language,
            content,
            name,
            editable,
        } => {
            props.insert("language".into(), language.clone());
            props.insert("content".into(), content.clone());
            props.insert("name".into(), name.clone());
            props.insert("editable".into(), editable.to_string());
        }
        ViewKind::SourceEditor { language, content } => {
            props.insert("language".into(), language.clone());
            props.insert("content".into(), content.clone());
        }
        ViewKind::BlockOperations { operations } => {
            props.insert("operations".into(), operations.clone());
        }
        ViewKind::PrefField {
            key,
            pref_type,
            value,
            requires_restart,
            locked,
            ..
        } => {
            props.insert("key".into(), key.clone());
            props.insert("pref_type".into(), pref_type.clone());
            props.insert("value".into(), format!("{value:?}"));
            props.insert("requires_restart".into(), requires_restart.to_string());
            props.insert("locked".into(), locked.to_string());
        }
        ViewKind::PieMenu { fields, .. } => {
            props.insert("fields".into(), fields.clone());
        }
        ViewKind::DropZone { op_name } => {
            props.insert("op_name".into(), op_name.clone());
        }
        ViewKind::ViewModeSwitcher {
            entity_uri, modes, ..
        } => {
            props.insert("entity_uri".into(), entity_uri.to_string());
            props.insert("modes".into(), modes.clone());
        }
        ViewKind::OpButton {
            op_name,
            target_id,
            display_name,
        } => {
            props.insert("op_name".into(), op_name.clone());
            props.insert("target_id".into(), target_id.clone());
            props.insert("display_name".into(), display_name.clone());
        }
        ViewKind::LiveQuery {
            query,
            query_lang,
            query_context_id,
            ..
        } => {
            if let Some(query) = query {
                props.insert("query".into(), query.clone());
            }
            if let Some(lang) = query_lang {
                props.insert("query_lang".into(), lang.clone());
            }
            if let Some(context) = query_context_id {
                props.insert("query_context_id".into(), context.clone());
            }
        }
        ViewKind::Unevaluated { mechanism, reason } => {
            props.insert("mechanism".into(), format!("{mechanism:?}"));
            props.insert("reason".into(), reason.clone());
        }
        ViewKind::TreeItem {
            depth,
            has_children,
            collapsed,
            ..
        } => {
            props.insert("depth".into(), depth.to_string());
            props.insert("has_children".into(), has_children.to_string());
            props.insert("collapsed".into(), collapsed.to_string());
        }
        // The row payload is the collection's own data and is addressable
        // through `entity_id`; copying it here would put every cell of every
        // table into the `snapshot_text` haystack that substring assertions
        // search.
        ViewKind::TableRow { .. } => {}
        // Structural kinds with no scalar field of their own.
        ViewKind::Divider
        | ViewKind::Tree { .. }
        | ViewKind::Outline { .. }
        | ViewKind::Table { .. }
        | ViewKind::QueryResult { .. }
        | ViewKind::Focusable { .. }
        | ViewKind::Selectable { .. }
        | ViewKind::Draggable { .. }
        | ViewKind::OnHover { .. }
        | ViewKind::BottomDock { .. }
        | ViewKind::LiveBlock { .. }
        | ViewKind::RenderBlock { .. }
        | ViewKind::Empty
        | ViewKind::Loading => {}
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
