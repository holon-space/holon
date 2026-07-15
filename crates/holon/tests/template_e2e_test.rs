use std::path::Path;
use std::sync::Arc;

use holon::api::BackendEngine;
use holon::core::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_providers;
use holon::storage::BLOCK_WRITE_TABLE;
use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_org_format::parse_org_file;
use holon_orgmode::build_block_params;

const TEMPLATES_ORG: &str = include_str!("../../../assets/default/Templates.org");

async fn block_engine() -> Arc<BackendEngine> {
    create_test_engine_with_providers(":memory:".into(), |module| {
        module.with_operation_provider_factory(|backend| {
            let db_handle =
                tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
            Arc::new(SqlOperationProvider::new(
                db_handle,
                BLOCK_WRITE_TABLE.to_string(),
                "block".to_string(),
                "block".to_string(),
            ))
        })
    })
    .await
    .unwrap()
}

fn instantiate_params(
    template_id: &str,
    target_parent: &str,
    context_key: &str,
    bindings: &[(&str, &str)],
) -> holon_api::StorageEntity {
    let mut params = holon_api::StorageEntity::new();
    params.insert("template_id".into(), Value::String(template_id.into()));
    params.insert("target_parent".into(), Value::String(target_parent.into()));
    params.insert("context_key".into(), Value::String(context_key.into()));
    if !bindings.is_empty() {
        params.insert(
            "bindings".into(),
            Value::Object(
                bindings
                    .iter()
                    .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
                    .collect(),
            ),
        );
    }
    params
}

fn str_field<'a>(row: &'a holon_api::StorageEntity, key: &str) -> &'a str {
    match row.get(key) {
        Some(Value::String(s)) => s,
        other => panic!("field '{key}': expected string, got {other:?}"),
    }
}

async fn query_children(engine: &BackendEngine, parent_id: &str) -> Vec<holon_api::StorageEntity> {
    engine
        .db_handle()
        .query(
            &format!(
                "SELECT * FROM block_raw WHERE parent_id = '{}' ORDER BY sort_key, id",
                parent_id.replace('\'', "''")
            ),
            std::collections::HashMap::new(),
        )
        .await
        .unwrap()
}

async fn query_by_id(engine: &BackendEngine, id: &str) -> Option<holon_api::StorageEntity> {
    let rows = engine
        .db_handle()
        .query(
            &format!(
                "SELECT * FROM block_raw WHERE id = '{}'",
                id.replace('\'', "''")
            ),
            std::collections::HashMap::new(),
        )
        .await
        .unwrap();
    rows.into_iter().next()
}

fn props_of(row: &holon_api::StorageEntity) -> serde_json::Value {
    match row.get("properties") {
        Some(Value::String(s)) | Some(Value::Json(s)) => {
            serde_json::from_str(s).expect("properties is valid JSON")
        }
        Some(obj @ Value::Object(_)) => serde_json::to_value(obj).unwrap(),
        other => panic!("properties: expected object or JSON string, got {other:?}"),
    }
}

/// Seed template blocks from a parsed org file. Returns the document block id
/// used as the routing doc.
async fn seed_templates(engine: &BackendEngine, parent_id: &str) -> Vec<String> {
    let path = Path::new("Templates.org");
    let root = Path::new("/vault");
    let parse_result = parse_org_file(
        path,
        TEMPLATES_ORG,
        &holon_api::entity_uri::EntityUri::no_parent(),
        root,
    )
    .expect("Templates.org must parse");

    let doc_id = parse_result.document.id.clone();
    let mut template_root_ids = Vec::new();

    for block in &parse_result.blocks {
        let effective_parent = if block.parent_id == doc_id {
            template_root_ids.push(block.id.as_str().to_string());
            holon_api::entity_uri::EntityUri::block(parent_id)
        } else {
            block.parent_id.clone()
        };
        let params = build_block_params(block, &effective_parent, &doc_id);
        engine
            .execute_operation(
                &EntityName::new("block"),
                "create",
                params,
                OpOrigin::Ingest,
            )
            .await
            .unwrap();
    }

    template_root_ids
}

fn has_non_null_marks(row: &holon_api::StorageEntity) -> bool {
    match row.get("marks") {
        Some(Value::String(s)) | Some(Value::Json(s)) => !s.is_empty(),
        Some(Value::Array(arr)) => !arr.is_empty(),
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn plan_my_day_instantiates_with_no_variables_and_preserves_marks() {
    let engine = block_engine().await;

    // Create target parent — a journal-day-shaped block.
    engine
        .execute_operation(
            &EntityName::new("block"),
            "create",
            {
                let mut p = holon_api::StorageEntity::new();
                p.insert("id".into(), Value::String("block:target".into()));
                p.insert("content".into(), Value::String("2026-07-15".into()));
                p
            },
            OpOrigin::User,
        )
        .await
        .unwrap();

    // Create the templates container block.
    engine
        .execute_operation(
            &EntityName::new("block"),
            "create",
            {
                let mut p = holon_api::StorageEntity::new();
                p.insert("id".into(), Value::String("block:__templates__".into()));
                p.insert("content".into(), Value::String("Templates".into()));
                p
            },
            OpOrigin::User,
        )
        .await
        .unwrap();

    let template_root_ids = seed_templates(&engine, "__templates__").await;
    let plan_my_day_id = template_root_ids
        .iter()
        .find(|id| id.contains("plan-my-day"))
        .expect("Plan my day template root must be seeded");
    assert!(
        plan_my_day_id.ends_with("plan-my-day-tpl-0"),
        "unexpected id: {plan_my_day_id}"
    );

    let entity = EntityName::new("block");

    // Instantiate with a fresh context key (no convergence).
    let context_key = format!(
        "pmd-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let result = engine
        .execute_operation(
            &entity,
            "instantiate_template",
            instantiate_params(plan_my_day_id, "block:target", &context_key, &[]),
            OpOrigin::Rule {
                transition_id: "rule:test-pmd".into(),
            },
        )
        .await
        .unwrap();
    let Some(Value::String(root_id)) = result else {
        panic!("instantiate_template must return the instance root id");
    };

    // The instance root content should be "Plan my day" (no variables).
    let root = query_by_id(&engine, &root_id)
        .await
        .expect("instance root must exist");
    assert_eq!(
        str_field(&root, "content"),
        "Plan my day",
        "root content preserved verbatim (no variables)"
    );
    let root_props = props_of(&root);
    assert_eq!(
        root_props["instance_of"].as_str(),
        Some(plan_my_day_id.as_str())
    );
    assert!(
        root_props.get("template").is_none(),
        "template marker stripped"
    );

    // Verify children: TODO task_state preserved on task lines,
    // bold marks present, links present.
    let children = query_children(&engine, &root_id).await;
    assert!(!children.is_empty(), "instance must have children");

    // Find the "Capture *open loops*" TODO task.
    let capture = children
        .iter()
        .find(|c| str_field(c, "content").contains("Capture"))
        .expect("'Capture open loops' child must exist");
    let cprops = props_of(capture);
    assert_eq!(
        cprops.get("task_state").and_then(|v| v.as_str()),
        Some("TODO"),
        "'Capture open loops' must carry task_state=TODO, props: {cprops:?}"
    );

    // Verify bold marks on a child with *bold* markup.
    let pomodoro = children
        .iter()
        .find(|c| str_field(c, "content").contains("Pomodoro"))
        .expect("'Start Pomodoro timer' child must exist");
    assert!(
        has_non_null_marks(pomodoro),
        "'Start Pomodoro timer' must carry marks for the bold *Pomodoro*"
    );

    // Verify link marks on the "Repeat Implementation Intentions" child.
    let repeat = children
        .iter()
        .find(|c| str_field(c, "content").contains("Implementation"))
        .expect("'Repeat Implementation Intentions' child must exist");
    assert!(
        has_non_null_marks(repeat),
        "'Repeat Implementation Intentions' must carry marks for the link"
    );

    // Second instantiation with same context key converges (no duplicates).
    engine
        .execute_operation(
            &entity,
            "instantiate_template",
            instantiate_params(plan_my_day_id, "block:target", &context_key, &[]),
            OpOrigin::Rule {
                transition_id: "rule:test-pmd".into(),
            },
        )
        .await
        .unwrap();
    let all_roots = query_children(&engine, "block:target").await;
    assert_eq!(
        all_roots.len(),
        1,
        "re-fire with same context_key must converge (no duplicates), \
         got {} children",
        all_roots.len()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn new_month_substitutes_date_and_is_idempotent() {
    let engine = block_engine().await;

    // Create target parent.
    engine
        .execute_operation(
            &EntityName::new("block"),
            "create",
            {
                let mut p = holon_api::StorageEntity::new();
                p.insert("id".into(), Value::String("block:target".into()));
                p.insert("content".into(), Value::String("2026-07-15".into()));
                p
            },
            OpOrigin::User,
        )
        .await
        .unwrap();

    // Create templates container.
    engine
        .execute_operation(
            &EntityName::new("block"),
            "create",
            {
                let mut p = holon_api::StorageEntity::new();
                p.insert("id".into(), Value::String("block:__templates__".into()));
                p.insert("content".into(), Value::String("Templates".into()));
                p
            },
            OpOrigin::User,
        )
        .await
        .unwrap();

    let template_root_ids = seed_templates(&engine, "__templates__").await;
    let new_month_id = template_root_ids
        .iter()
        .find(|id| id.contains("new-month"))
        .expect("NewMonth template root must be seeded");
    assert!(
        new_month_id.ends_with("new-month-tpl-0"),
        "unexpected id: {new_month_id}"
    );

    let entity = EntityName::new("block");
    let day_key = "2026-07-15";

    // First instantiation with date binding.
    let result = engine
        .execute_operation(
            &entity,
            "instantiate_template",
            instantiate_params(
                new_month_id,
                "block:target",
                day_key,
                &[("date", "2026-07-15")],
            ),
            OpOrigin::Rule {
                transition_id: "rule:test-nm".into(),
            },
        )
        .await
        .unwrap();
    let Some(Value::String(root_id)) = result else {
        panic!("instantiate_template must return the instance root id");
    };

    // Verify heading substitution: root content = "Monatswechsel 2026-07-15".
    let root = query_by_id(&engine, &root_id)
        .await
        .expect("instance root must exist");
    assert_eq!(
        str_field(&root, "content"),
        "Monatswechsel 2026-07-15",
        "{{date}} in heading must be substituted"
    );
    let root_props = props_of(&root);
    assert_eq!(
        root_props["instance_of"].as_str(),
        Some(new_month_id.as_str())
    );

    // Verify property substitution: :text: Monatswechsel {{date}}.
    assert_eq!(
        root_props.get("text").and_then(|v| v.as_str()),
        Some("Monatswechsel 2026-07-15"),
        "{{date}} in text property must be substituted"
    );

    // Verify TODO children exist with task_state.
    let children = query_children(&engine, &root_id).await;
    let desired_outcomes = children
        .iter()
        .find(|c| str_field(c, "content").contains("Desired Outcomes"))
        .expect("'Desired Outcomes' heading must exist");
    let tasks_heading = children
        .iter()
        .find(|c| str_field(c, "content").contains("Tasks"))
        .expect("'Tasks' heading must exist");

    // Verify sibling order: Desired Outcomes before Tasks.
    let desired_content = str_field(&desired_outcomes, "content");
    let tasks_content = str_field(&tasks_heading, "content");
    let sort_keys: Vec<String> = children
        .iter()
        .map(|c| {
            c.get("sort_key")
                .and_then(|v| v.as_string())
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let desired_idx = children
        .iter()
        .position(|c| str_field(c, "content") == desired_content)
        .unwrap();
    let tasks_idx = children
        .iter()
        .position(|c| str_field(c, "content") == tasks_content)
        .unwrap();
    assert!(
        desired_idx < tasks_idx,
        "sibling order: Desired Outcomes (idx {desired_idx}) must precede \
         Tasks (idx {tasks_idx}), sort_keys: {sort_keys:?}"
    );

    // Verify a TODO task under Tasks exists with task_state=TODO.
    let tasks_children = query_children(&engine, &str_field(&tasks_heading, "id")).await;
    assert!(!tasks_children.is_empty(), "Tasks must have TODO children");
    let first_task = &tasks_children[0];
    let ft_props = props_of(first_task);
    assert_eq!(
        ft_props.get("task_state").and_then(|v| v.as_str()),
        Some("TODO"),
        "first task under Tasks must carry task_state=TODO, props: {ft_props:?}"
    );

    // Verify link mark on Google AI Rechnung child.
    // The link child is under "LexOffice von ältestem zu neuestem Eintrag
    // durchgehen" which is under "Buchungen in LexOffice zuordnen".
    // Walk the tree to find it.
    let buchungen = tasks_children
        .iter()
        .find(|c| str_field(c, "content").contains("Buchungen in LexOffice"))
        .expect("'Buchungen in LexOffice' task must exist");
    let buchungen_children = query_children(&engine, &str_field(&buchungen, "id")).await;
    let lex_office = buchungen_children
        .iter()
        .find(|c| str_field(c, "content").contains("LexOffice von"))
        .expect("'LexOffice von altestem' task must exist");
    let lex_children = query_children(&engine, &str_field(&lex_office, "id")).await;
    let google_link = lex_children
        .iter()
        .find(|c| str_field(c, "content").contains("Google AI"))
        .expect("'Google AI Rechnung downloaden' link child must exist");
    assert!(
        has_non_null_marks(google_link),
        "'Google AI Rechnung' must carry marks for the link"
    );

    // Second run with same day_key converges (no duplicates).
    engine
        .execute_operation(
            &entity,
            "instantiate_template",
            instantiate_params(
                new_month_id,
                "block:target",
                day_key,
                &[("date", "2026-07-15")],
            ),
            OpOrigin::Rule {
                transition_id: "rule:test-nm".into(),
            },
        )
        .await
        .unwrap();
    let all_roots = query_children(&engine, "block:target").await;
    assert_eq!(
        all_roots.len(),
        1,
        "re-fire with same context_key must converge (no duplicates), \
         got {} children",
        all_roots.len()
    );
}
