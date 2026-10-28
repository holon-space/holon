//! A type whose profile is malformed (a variant condition that cannot be split,
//! a render that does not parse) is refused and disclosed on the condition bus;
//! the session boots, the type keeps its schema, and every other profile loads
//! (ruling PROFILE-panic.a).
//!
//! @pbt kind harness
//! @pbt covers malformed-type-profile-boots-degraded — a malformed type profile
//! is disclosed as a condition and never applied, and boot continues
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone registers no
//! malformed type

use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use holon_api::MalformedTypeProfile;
use holon_api::TypeDefinition;
use holon_api::Value;
use holon_api::VariantPart;
use holon_api::render_types::RenderProfile;
use holon_conditions::ConditionBus;
use holon_conditions::ConditionKind;
use holon_frontend::config::HolonConfig;
use holon_frontend::config::SessionConfig;
use holon_frontend::config::VaultConfig;
use holon_profiles::TypeRegistry;

fn type_yaml(name: &str, condition: &str, render: &str) -> TypeDefinition {
    let yaml = format!(
        "\
name: {name}
default_lifetime: persistent
primary_key: id
fields:
  - name: id
    sql_type: TEXT
    primary_key: true
  - name: name
    sql_type: TEXT
  - name: properties
    sql_type: TEXT
    nullable: true
    jsonb: true
    value_kind: overflow_properties
  - name: property_kinds
    sql_type: TEXT
    nullable: true
    value_kind: overflow_property_kinds
profile_variants:
  - name: {name}_card
    priority: 5
    condition: '{condition}'
    render: '{render}'
"
    );
    serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("{name}: {e}\n{yaml}"))
}

const GOOD_RENDER: &str = r#"text(col("name"))"#;
const UNSPLITTABLE_CONDITION: &str = "flagged || archived && is_focused";
const UNPARSEABLE_RENDER: &str = r#"text(col("name")"#;

fn register_types(registry: &TypeRegistry) {
    for type_def in [
        type_yaml("widget", "name != ()", GOOD_RENDER),
        type_yaml("gadget", UNSPLITTABLE_CONDITION, GOOD_RENDER),
        type_yaml("gizmo", "name != ()", UNPARSEABLE_RENDER),
    ] {
        registry.register(type_def).expect("register the test type");
    }
}

struct Booted {
    engine: Arc<holon::api::BackendEngine>,
    bus: Arc<ConditionBus>,
    _session: Arc<holon_frontend::FrontendSession>,
}

async fn boot(dir: &Path) -> Booted {
    let holon_config = HolonConfig {
        db_path: Some(dir.join("malformed-type-profile.db")),
        vault: VaultConfig {
            root: Some(dir.to_path_buf()),
        },
        ..Default::default()
    };
    let (session, engine, bus) = holon_app::new_from_config_with_di(
        holon_config,
        SessionConfig::new(holon_api::UiInfo::permissive()),
        dir.to_path_buf(),
        HashSet::new(),
        Arc::new(ConditionBus::new()),
        |injector| {
            holon::testing::database_stuck_guard::report_database_stuck_in(injector);
            register_types(&injector.resolve::<TypeRegistry>());
            Ok(())
        },
        |injector| (*injector.resolve::<Arc<ConditionBus>>()).clone(),
    )
    .await
    .expect("a malformed type profile is the user's to fix, so the session must boot");
    Booted {
        engine,
        bus,
        _session: session,
    }
}

fn resolve(engine: &holon::api::BackendEngine, id: &str) -> Arc<RenderProfile> {
    let row: HashMap<String, Value> = HashMap::from([
        ("id".to_string(), Value::String(id.into())),
        ("name".to_string(), Value::String("probe".into())),
        ("content".to_string(), Value::String("probe".into())),
    ]);
    engine.profile_resolver().resolve(&row)
}

/// The resolver's answer for an entity it holds no profile for.
fn is_profileless(profile: &RenderProfile) -> bool {
    matches!(
        &profile.render,
        holon_api::RenderExpr::Literal { value: Value::String(s) } if s.is_empty()
    ) && profile.variants.is_empty()
}

#[test]
fn a_malformed_type_profile_is_refused_and_disclosed_and_boot_continues() {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build test runtime")
        .block_on(async {
            let dir = tempfile::tempdir().expect("tempdir");
            let booted = boot(dir.path()).await;

            let refusals: HashMap<String, MalformedTypeProfile> = booted
                .bus
                .current()
                .into_iter()
                .filter_map(|c| match c.reason {
                    ConditionKind::TypeProfileRefused(refusal) => Some((c.subject, refusal)),
                    _ => None,
                })
                .collect();
            assert_eq!(
                refusals.keys().cloned().collect::<HashSet<_>>(),
                HashSet::from(["gadget".to_string(), "gizmo".to_string()]),
                "exactly the two malformed type profiles must be disclosed: {refusals:#?}"
            );
            for (entity, part, text) in [
                ("gadget", VariantPart::Condition, UNSPLITTABLE_CONDITION),
                ("gizmo", VariantPart::Render, UNPARSEABLE_RENDER),
            ] {
                let refusal = &refusals[entity];
                assert_eq!(
                    (
                        refusal.entity.as_str(),
                        refusal.variant.as_str(),
                        refusal.part,
                        refusal.text.as_str()
                    ),
                    (entity, format!("{entity}_card").as_str(), part, text),
                    "the disclosure must name the type, variant, part and text: {refusal}"
                );
                let detail = ConditionKind::TypeProfileRefused(refusal.clone()).detail(entity);
                let shown = format!("{}\n{}", detail.headline, detail.body.join("\n"));
                for needle in [entity, text, refusal.reason.as_str()] {
                    assert!(
                        shown.contains(needle),
                        "the shown detail must carry {needle:?}: {shown}"
                    );
                }
            }
            assert!(
                refusals["gadget"].reason.contains("`||`"),
                "the reason must say why the condition does not split: {}",
                refusals["gadget"]
            );

            for refused in ["gadget:g1", "gizmo:g1"] {
                assert!(
                    is_profileless(&resolve(&booted.engine, refused)),
                    "a refused type profile must not be applied to {refused}"
                );
            }
            assert_eq!(
                resolve(&booted.engine, "widget:w1").name,
                "widget_card",
                "a well-formed type profile beside a refused one must load"
            );
            assert!(
                !is_profileless(&resolve(&booted.engine, "block:b1")),
                "the bundled block profile must load beside a refused one"
            );
        });
}
