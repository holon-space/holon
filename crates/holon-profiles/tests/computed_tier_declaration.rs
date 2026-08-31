//! The declared tier — not the expression's shape — decides a computed field's
//! seat, and a `computed_persisted` field that cannot lower to SQL is refused
//! at registry load.

use holon_api::ComputedTier;
use holon_api::Value;
use holon_api::computation::Computation;
use holon_profiles::TypeRegistry;
use holon_profiles::parse_profile_yaml;

const PERSON_TYPE: &str = include_str!("../../../assets/default/types/person.yaml");

fn registry_with_person() -> TypeRegistry {
    let registry = TypeRegistry::new();
    let type_def: holon_api::TypeDefinition =
        serde_yaml::from_str(PERSON_TYPE).expect("person.yaml");
    registry.register(type_def).expect("register person");
    registry
}

#[test]
fn a_computed_live_field_evaluates_without_turso() {
    let registry = registry_with_person();
    registry
        .apply_parsed_profile(
            parse_profile_yaml(
                "entity_name: person\ncomputed:\n  greeting:\n    tier: computed_live\n    expr: \
                 '\"hi \" + email'\n",
            )
            .expect("profile parses"),
        )
        .expect("live tier accepted");

    let td = registry.get("person").expect("person registered");
    let spec = td.computed_spec("greeting").expect("greeting declared");
    assert_eq!(spec.tier(), ComputedTier::ComputedLive);

    let ctx = [("email".to_string(), Value::String("a@b.c".into()))]
        .into_iter()
        .collect();
    assert_eq!(
        spec.computation().eval(&ctx).expect("evaluates in memory"),
        Value::String("hi a@b.c".into()),
        "the live tier must evaluate with no database present"
    );
}

#[test]
fn a_computed_persisted_field_that_cannot_lower_to_sql_is_refused_at_load() {
    let registry = registry_with_person();
    let err = registry
        .apply_parsed_profile(
            parse_profile_yaml(
                "entity_name: person\ncomputed:\n  tagged:\n    tier: computed_persisted\n    \
                 expr: 'role.contains(\"x\")'\n",
            )
            .expect("profile parses"),
        )
        .expect_err("a persisted field outside the SQL subset must be refused");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("tagged"),
        "the error must name the field, got: {msg}"
    );
    assert!(
        msg.contains("computed_persisted"),
        "the error must name the tier it violated, got: {msg}"
    );
}

#[test]
fn declared_text_columns_make_plus_a_concat() {
    let registry = registry_with_person();
    registry
        .apply_parsed_profile(
            parse_profile_yaml(
                "entity_name: person\ncomputed:\n  who:\n    tier: computed_persisted\n    expr: \
                 'role + email'\n",
            )
            .expect("profile parses"),
        )
        .expect("two TEXT columns concatenate");

    let td = registry.get("person").expect("person registered");
    let spec = td.computed_spec("who").expect("who declared");
    assert!(
        matches!(spec.computation(), Computation::Concat { .. }),
        "`role + email` over two declared TEXT columns must be Concat, got {:?}",
        spec.computation()
    );
}

#[test]
fn an_out_of_subset_live_field_is_disclosed_as_rhai_only() {
    let registry = registry_with_person();
    registry
        .apply_parsed_profile(
            parse_profile_yaml(
                "entity_name: person\ncomputed:\n  tagged: 'role.contains(\"x\")'\n",
            )
            .expect("profile parses"),
        )
        .expect("the live tier accepts a Rhai-only expression");

    let td = registry.get("person").expect("person registered");
    assert_eq!(
        td.rhai_only_computed_fields(),
        vec!["tagged"],
        "a field served only by Rhai must be visible to the certifier"
    );
}

/// A computed field's name reaches SQL in identifier position, so it is parsed
/// at declaration time. These are the shapes that would otherwise break out of
/// the `<expr> AS <name>` slot.
#[test]
fn a_field_name_that_is_not_an_identifier_is_refused_at_load() {
    for hostile in [
        r#"display_name" AS x, (SELECT 1) AS pwned"#,
        "x, 1 AS injected",
        "name; DROP TABLE person",
        "a--b",
    ] {
        let registry = registry_with_person();
        let yaml = format!(
            "entity_name: person\ncomputed:\n  ? {}\n  : {{tier: computed_persisted, expr: email}}\n",
            serde_yaml::to_string(hostile).expect("quote key").trim()
        );
        let parsed = parse_profile_yaml(&yaml)
            .unwrap_or_else(|e| panic!("yaml for `{hostile}` must parse: {e}\n{yaml}"));
        let err = registry
            .apply_parsed_profile(parsed)
            .expect_err("a non-identifier field name must be refused");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not a bare identifier") && msg.contains(hostile),
            "the refusal must name the offending field, got: {msg}"
        );
    }
}

/// A SQL keyword is a legal column name — the generated DDL quotes identifiers
/// (`create_table_keyword_columns`), so it is accepted, not refused.
#[test]
fn a_sql_keyword_field_name_is_accepted() {
    let registry = registry_with_person();
    let yaml = "entity_name: person\ncomputed:\n  order: {tier: computed_persisted, expr: email}\n";
    registry
        .apply_parsed_profile(parse_profile_yaml(yaml).expect("profile parses"))
        .expect("a keyword-named computed field is accepted");
}

/// The Rhai-only fallback must stay audible: silencing it would leave a
/// degraded field indistinguishable from a planted one.
#[test]
fn the_rhai_only_fallback_is_disclosed_in_the_log() {
    use std::sync::Arc;
    use std::sync::Mutex;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<String>>);
    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _: tracing_subscriber::layer::Context<'_, S>,
        ) {
            struct V<'a>(&'a mut String);
            impl tracing::field::Visit for V<'_> {
                fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
                    self.0.push_str(&format!(" {}={v:?}", f.name()));
                }
            }
            let mut buf = self.0.lock().expect("capture");
            buf.push_str(&format!("[{}]", event.metadata().level()));
            event.record(&mut V(&mut buf));
            buf.push('\n');
        }
    }

    let capture = Capture::default();
    let sink = capture.0.clone();
    let subscriber = {
        use tracing_subscriber::layer::SubscriberExt;
        tracing_subscriber::registry().with(capture)
    };

    tracing::subscriber::with_default(subscriber, || {
        let registry = registry_with_person();
        registry
            .apply_parsed_profile(
                parse_profile_yaml(
                    "entity_name: person\ncomputed:\n  tagged: 'role.contains(\"x\")'\n",
                )
                .expect("profile parses"),
            )
            .expect("live tier accepts Rhai-only");
    });

    let logged = sink.lock().expect("capture").clone();
    assert!(
        logged.contains("WARN"),
        "the Rhai-only fallback must be disclosed at WARN, captured:\n{logged}"
    );
    assert!(
        logged.contains("tagged"),
        "the disclosure must name the field, captured:\n{logged}"
    );
}

/// The types decide the operator, so a spec whose declared types disagree with
/// the columns it is attached to would split the seats: SQL adds where `eval`
/// raises.
#[test]
fn a_spec_whose_types_contradict_the_type_definition_is_refused() {
    use holon_api::FieldLifetime;
    use holon_api::FieldSchema;
    use holon_api::computation::FieldKind;
    use holon_api::computation::FieldTypes;

    let mut tampered = FieldTypes::new();
    tampered.insert("role", FieldKind::Numeric);
    tampered.insert("email", FieldKind::Numeric);
    let spec = holon_api::ComputedSpec::parse(
        "who",
        "role + email",
        ComputedTier::ComputedPersisted,
        &tampered,
        &rhai::Engine::new(),
    )
    .expect("parses against the types it was handed");

    let mut type_def: holon_api::TypeDefinition =
        serde_yaml::from_str(PERSON_TYPE).expect("person.yaml");
    type_def.fields.push(FieldSchema {
        name: "who".to_string(),
        sql_type: "TEXT".to_string(),
        lifetime: FieldLifetime::Computed { spec },
        ..Default::default()
    });

    let err = TypeRegistry::new()
        .register(type_def)
        .expect_err("types contradicting the owning type must be refused");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("who") && (msg.contains("role") || msg.contains("email")),
        "the refusal must name the field and the contradicted column, got: {msg}"
    );
}
