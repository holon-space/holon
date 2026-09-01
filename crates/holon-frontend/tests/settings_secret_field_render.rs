//! The shopping list URL reaches the settings surface as a MASKED field.
//!
//! The URL contains the list's capability token, so what the Settings screen
//! must carry to a renderer is `pref_type: "secret"` — that discriminant is the
//! only thing standing between a stored credential and a plaintext row. The
//! whole preferences tree is interpreted here, through the production shadow
//! builder, rather than the one node: a field the schema declares but the
//! render expression never emits is invisible to the user, and this is the tier
//! that can tell those apart.
//!
//! @pbt kind harness
//! @pbt covers settings-secret-field-render — the shopping list URL is emitted
//! into the preferences render tree and carries the secret discriminant, and an
//! environment-shadowed row carries the read-only flag
//! @pbt slips-if-removed a credential preference renders as a plaintext text
//! field, or never renders at all

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::Value;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::preferences::PrefKey;
use holon_frontend::preferences::define_preferences;
use holon_frontend::preferences::env_shadowed_keys;
use holon_frontend::preferences::preferences_render_expr;
use holon_frontend::preferences::preferences_to_rows;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::theme::ThemeRegistry;

const PREF_KEY: &str = "shopping.list_url";

/// Synthetic. A real list URL is a live credential and never belongs in a
/// fixture.
const STORED_URL: &str = "https://shop.example/c/abc123SYNTHETICrenderTOKEN/api";

/// Interpret the whole preferences tree the settings screen renders.
fn settings_tree(shadowing_env: bool) -> Arc<ReactiveViewModel> {
    let defs = define_preferences(&ThemeRegistry::load(None));
    let current = HashMap::from([(
        PrefKey::new(PREF_KEY),
        toml::Value::String(STORED_URL.into()),
    )]);
    let locked = env_shadowed_keys(&defs, &|name| {
        (shadowing_env && name == "SHOPPING_LIST_URL").then(|| STORED_URL.to_string())
    });

    let rows: Vec<Arc<DataRow>> = preferences_to_rows(&defs, &current, &locked)
        .into_iter()
        .map(Arc::new)
        .collect();
    let ctx = RenderContext::default().with_data_rows(rows);
    Arc::new(StubBuilderServices::new().interpret(&preferences_render_expr(&defs), &ctx))
}

/// The interpreted `pref_field` node for `key`, wherever the section grouping
/// put it.
fn field(vm: &Arc<ReactiveViewModel>, key: &str) -> Arc<ReactiveViewModel> {
    fn walk(node: &Arc<ReactiveViewModel>, key: &str, found: &mut Vec<Arc<ReactiveViewModel>>) {
        let is_match = node.widget_name().as_deref() == Some("pref_field")
            && node.props.lock_ref().get("key") == Some(&Value::String(key.into()));
        if is_match {
            found.push(node.clone());
        }
        for child in &node.children {
            walk(child, key, found);
        }
    }
    let mut found = Vec::new();
    walk(vm, key, &mut found);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one {key:?} field in the settings tree, found {}",
        found.len()
    );
    found.remove(0)
}

#[test]
fn the_shopping_list_url_renders_as_a_secret_field() {
    let node = field(&settings_tree(false), PREF_KEY);
    assert_eq!(
        node.props.lock_ref().get("pref_type"),
        Some(&Value::String("secret".into())),
        "the discriminant a renderer masks on — without it the stored capability token is drawn \
         as ordinary text"
    );
}

#[test]
fn an_unshadowed_secret_field_stays_the_users_to_set() {
    let node = field(&settings_tree(false), PREF_KEY);
    assert_eq!(
        node.props.lock_ref().get("locked"),
        Some(&Value::Boolean(false)),
        "with nothing exported the settings menu owns this value — needing an export is exactly \
         what this field removes"
    );
}

#[test]
fn an_environment_shadowed_secret_field_renders_read_only() {
    let node = field(&settings_tree(true), PREF_KEY);
    assert_eq!(
        node.props.lock_ref().get("locked"),
        Some(&Value::Boolean(true)),
        "an export outranks the field; an editable row holding a credential nothing reads leaves \
         the user unable to tell which of two secrets is in force"
    );
}
