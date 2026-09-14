//! Differential: the sidecar's `response` mapping ≡ the bespoke Rust parse it
//! replaces.
//!
//! The model below is the parse that lived in `holon_kitchen::shopping` before
//! the low-code increments, transcribed verbatim. The SUT is the shipped
//! sidecar's jaq filter fed through [`RowMapper`] and the GENERIC list path —
//! the connection the sidecar's own `holon.list_sync` block declares. Both
//! are projected onto [`Observed`] — the public shape a consumer can actually
//! see — so a difference in private structure is not mistaken for a difference
//! in behaviour.
//!
//! **The divergence rule** (`plan-lowcode-connections.md` §4): a divergence is
//! NAMED and triaged as a bugfunnel entry — old wrong, new wrong, or both
//! wrong — never allowlisted silently. This test passes when the divergence set
//! is empty. It is the gate that makes deleting the old parse safe.

use std::collections::BTreeMap;

use anyhow::Result;
use holon_connections::CompiledListSync;
use holon_connections::ListSnapshot;
use holon_connections::ListSyncSpec;
use holon_core::file_format::TypedRowSet;
use holon_rows::RowMapper;
use proptest::prelude::*;
use serde_json::Value as Json;

const FETCHED_AT: &str = "2026-09-03T10:00:00Z";

/// The shipped sidecar. Reading the filter from the asset rather than restating
/// it here is what makes this a test OF the connection: a filter edited in the
/// sidecar is the filter this exercises.
const SIDECAR: &str = include_str!("../../../assets/integrations/shopping.yaml");

fn response_filter() -> String {
    let doc: serde_yaml::Value = serde_yaml::from_str(SIDECAR).expect("the sidecar parses");
    doc["holon"]["tools"]["pull_list"]["response"]
        .as_str()
        .expect("holon.tools.pull_list.response is a jaq filter")
        .to_string()
}

// ---------------------------------------------------------------- observable

/// What a consumer of a snapshot can see. Both sides are compared as this.
#[derive(Debug, Clone, PartialEq)]
struct Observed {
    version: (i64, i64),
    /// code → (icon, color)
    cats: BTreeMap<String, (Option<String>, Option<String>)>,
    /// (name, cat) → (count, checked, the category resolved)
    items: BTreeMap<(String, String), (Option<f64>, bool, bool)>,
}

/// The connection this sidecar declares, compiled against the shipped
/// `shopping_item` type. Reading BOTH from the assets is what makes this a test
/// of the shopping connection rather than of a restatement of it.
fn connection() -> std::sync::Arc<CompiledListSync> {
    let sidecar: serde_yaml::Value = serde_yaml::from_str(SIDECAR).expect("the sidecar parses");
    let spec: ListSyncSpec = serde_yaml::from_value(sidecar["holon"]["list_sync"].clone())
        .expect("the sidecar declares a list_sync block");
    let declared = holon_kitchen::shopping_item_type().expect("the declared type");
    CompiledListSync::compile("shopping", spec, &declared).expect("the connection compiles")
}

/// The category vocabulary, read from the rows the mapping emits for it. The
/// generic list path carries the ITEM rows and the version envelope; the
/// categories are a second declared type, written alongside.
fn observed_cats(rows: &[TypedRowSet]) -> BTreeMap<String, (Option<String>, Option<String>)> {
    rows.iter()
        .filter(|row_set| row_set.type_name == "shopping_category")
        .flat_map(|row_set| row_set.rows.iter())
        .map(|row| {
            let text = |column: &str| match row.get(column) {
                Some(holon_api::Value::String(s)) => Some(s.clone()),
                _ => None,
            };
            (
                text("code").expect("a category row carries its code"),
                (text("icon"), text("color")),
            )
        })
        .collect()
}

fn as_f64(value: Option<&holon_api::Value>) -> Option<f64> {
    match value {
        Some(holon_api::Value::Integer(n)) => Some(*n as f64),
        Some(holon_api::Value::Float(f)) => Some(*f),
        _ => None,
    }
}

fn observe(rows: &[TypedRowSet], snapshot: &ListSnapshot) -> Observed {
    let cats = observed_cats(rows);
    let picked = match snapshot.envelope().get("picked_items_version") {
        Some(holon_api::Value::Integer(n)) => *n,
        other => panic!("the list row carries no whole `picked_items_version` (got {other:?})"),
    };
    Observed {
        items: snapshot
            .rows()
            .map(|(_, item)| {
                let text = |column: &str| match item.columns.get(column) {
                    Some(holon_api::Value::String(s)) => s.clone(),
                    other => panic!("an item row's `{column}` is not text (got {other:?})"),
                };
                let cat = text("cat");
                // A latch arrives from a mapping as either a boolean or a
                // number, and the column it lands in is `INTEGER`, so both
                // spellings of "on" count here.
                let checked = matches!(
                    item.columns.get("checked"),
                    Some(holon_api::Value::Boolean(true)) | Some(holon_api::Value::Integer(1))
                );
                (
                    (text("name"), cat.clone()),
                    (
                        as_f64(item.columns.get("count")),
                        checked,
                        cats.contains_key(&cat),
                    ),
                )
            })
            .collect(),
        version: (snapshot.version(), picked),
        cats,
    }
}

// -------------------------------------------------------------------- the SUT

fn through_the_sidecar(
    mapper: &RowMapper,
    compiled: &CompiledListSync,
    body: &Json,
) -> Result<Observed> {
    let rows = mapper.map_to_row_sets(body)?;
    let snapshot = ListSnapshot::from_rows(compiled, &rows, FETCHED_AT)?;
    Ok(observe(&rows, &snapshot))
}

// ------------------------------------------------------------------ the model

/// The parse this increment deletes, transcribed. Nothing here may be
/// "improved": its job is to state what the old behaviour WAS.
mod model {
    use std::collections::BTreeMap;

    use anyhow::Result;
    use serde_json::Map;
    use serde_json::Value as Json;

    use super::Observed;

    #[derive(Clone)]
    struct Entry {
        code: String,
        icon: Option<String>,
        color: Option<String>,
    }

    fn parse_entry(raw: &str) -> Entry {
        let mut segments = raw.split('_');
        let code = segments.next().unwrap_or_default().to_string();
        Entry {
            code,
            icon: segments.next().map(str::to_string),
            color: segments.next().map(str::to_string),
        }
    }

    struct Item {
        name: String,
        cat: String,
        count: Option<f64>,
        checked: bool,
    }

    pub fn parse(response: &Map<String, Json>) -> Result<Observed> {
        let by_code = parse_vocabulary(response)?;
        let list = required_int(response, "version")?;
        let picked = match response.get("pickedItemsVersion") {
            None | Some(Json::Null) => required_int(response, "version")?,
            Some(_) => required_int(response, "pickedItemsVersion")?,
        };

        let mut items: BTreeMap<(String, String), Item> = BTreeMap::new();
        for value in required_array(response, "items")? {
            let record = value
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("`items` holds a non-object entry"))?;
            fold(
                &mut items,
                Item {
                    name: required_name(record.get("name"))?,
                    cat: required_cat(record.get("cat"))?,
                    count: optional_count(record.get("count"))?,
                    checked: false,
                },
            );
        }
        for (name, value) in required_object(response, "pickedItems")? {
            let record = value
                .as_object()
                .ok_or_else(|| anyhow::anyhow!("`pickedItems['{name}']` is not an object"))?;
            fold(
                &mut items,
                Item {
                    name: required_name(Some(&Json::String(name.to_string())))?,
                    cat: required_cat(record.get("cat"))?,
                    count: optional_count(record.get("count"))?,
                    checked: true,
                },
            );
        }

        Ok(Observed {
            version: (list, picked),
            cats: by_code
                .iter()
                .map(|(code, e)| (code.clone(), (e.icon.clone(), e.color.clone())))
                .collect(),
            items: items
                .into_iter()
                .map(|(key, item)| {
                    (
                        key.clone(),
                        (item.count, item.checked, by_code.contains_key(&key.1)),
                    )
                })
                .collect(),
        })
    }

    fn fold(items: &mut BTreeMap<(String, String), Item>, item: Item) {
        let key = (item.name.clone(), item.cat.clone());
        match items.get_mut(&key) {
            Some(held) => {
                held.count = Some(held.count.unwrap_or(1.0) + item.count.unwrap_or(1.0));
                held.checked |= item.checked;
            }
            None => {
                items.insert(key, item);
            }
        }
    }

    fn parse_vocabulary(response: &Map<String, Json>) -> Result<BTreeMap<String, Entry>> {
        let options = required_object(response, "options")?;
        let cats = required_array(options, "cats")?;
        let mut by_code = BTreeMap::new();
        for value in cats {
            let raw = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("`options.cats` holds a non-string entry"))?;
            let entry = parse_entry(raw);
            anyhow::ensure!(
                !entry.code.is_empty(),
                "`options.cats` holds an entry with no category code: '{raw}'"
            );
            anyhow::ensure!(
                by_code.insert(entry.code.clone(), entry).is_none(),
                "`options.cats` declares the category code twice: '{raw}'"
            );
        }
        Ok(by_code)
    }

    fn required_name(value: Option<&Json>) -> Result<String> {
        match value {
            Some(Json::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
            Some(other) => anyhow::bail!("`name` must be a non-empty string, got {other}"),
            None => anyhow::bail!("`name` is missing"),
        }
    }

    fn required_cat(value: Option<&Json>) -> Result<String> {
        match value {
            Some(Json::String(s)) if !s.trim().is_empty() => Ok(s.clone()),
            Some(other) => anyhow::bail!("`cat` must be a non-empty string, got {other}"),
            None => anyhow::bail!("`cat` is missing"),
        }
    }

    fn optional_count(value: Option<&Json>) -> Result<Option<f64>> {
        match value {
            None | Some(Json::Null) => Ok(None),
            Some(Json::Number(n)) => {
                Ok(Some(n.as_f64().ok_or_else(|| {
                    anyhow::anyhow!("`count` is not representable")
                })?))
            }
            Some(other) => anyhow::bail!("`count` must be a number, got {other}"),
        }
    }

    fn required_array<'a>(object: &'a Map<String, Json>, key: &str) -> Result<&'a Vec<Json>> {
        object
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("the list response has no `{key}`"))?
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("`{key}` is not an array"))
    }

    fn required_object<'a>(
        object: &'a Map<String, Json>,
        key: &str,
    ) -> Result<&'a Map<String, Json>> {
        object
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("the list response has no `{key}`"))?
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("`{key}` is not an object"))
    }

    fn required_int(object: &Map<String, Json>, key: &str) -> Result<i64> {
        object
            .get(key)
            .ok_or_else(|| anyhow::anyhow!("the list response has no `{key}`"))?
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("`{key}` is not a whole number"))
    }
}

// --------------------------------------------------------------- the compare

/// Both sides classified the same way, so an Err on one side and an Ok on the
/// other is a divergence rather than a panic. Messages are NOT compared: the
/// mapping speaks jq's diagnostics, and requiring identical prose would pin
/// wording rather than behaviour.
fn compare(
    mapper: &RowMapper,
    compiled: &CompiledListSync,
    body: &Json,
) -> std::result::Result<(), String> {
    let object = body.as_object().expect("a response body is an object");
    let expected = model::parse(object);
    let actual = through_the_sidecar(mapper, compiled, body);
    match (&expected, &actual) {
        (Ok(a), Ok(b)) if a == b => Ok(()),
        (Err(_), Err(_)) => Ok(()),
        (Ok(a), Ok(b)) => Err(format!(
            "both accepted, differently\n  old: {a:?}\n  new: {b:?}\n  body: {body}"
        )),
        (Ok(a), Err(e)) => Err(format!(
            "the old parse accepted and the mapping refused\n  old: {a:?}\n  new err: {e:#}\n  \
             body: {body}"
        )),
        (Err(e), Ok(b)) => Err(format!(
            "the old parse refused and the mapping accepted\n  old err: {e:#}\n  new: {b:?}\n  \
             body: {body}"
        )),
    }
}

// ------------------------------------------------------------ captured shapes

/// The shapes `utcp-shopping/api-spec.md` §2 pins from the live peer, plus the
/// boundary cases the old parse's own tests cover.
fn captured() -> Vec<Json> {
    vec![
        serde_json::json!({
            "items": [{"name": "Milk", "cat": "R", "count": 2}, {"name": "Bread", "cat": "B"}],
            "pickedItems": {"Rice": {"cat": "Ca", "date": "2026-09-01T08:00:00Z"}},
            "version": 7,
            "options": {"prices": false, "cats": ["R", "B", "Ca", "Ir", "Kleidung_clothes_1976D2"]}
        }),
        // An empty list is legitimate and must not be an error.
        serde_json::json!({"items": [], "pickedItems": {}, "version": 0, "options": {"cats": []}}),
        // `pickedItemsVersion` present, and present as null.
        serde_json::json!({"items": [], "pickedItems": {}, "version": 3,
                           "pickedItemsVersion": 9, "options": {"cats": []}}),
        serde_json::json!({"items": [], "pickedItems": {}, "version": 3,
                           "pickedItemsVersion": null, "options": {"cats": []}}),
        // The same key twice: counts fold, checked wins.
        serde_json::json!({"items": [{"name": "Milk", "cat": "R"}, {"name": "Milk", "cat": "R", "count": 4}],
                           "pickedItems": {"Milk": {"cat": "R"}}, "version": 1,
                           "options": {"cats": ["R"]}}),
        // A cat outside the published vocabulary must not fail the fetch.
        serde_json::json!({"items": [{"name": "Milk", "cat": "ZZ"}], "pickedItems": {},
                           "version": 1, "options": {"cats": ["R"]}}),
        // Refusals the old parse made.
        serde_json::json!({"items": [], "pickedItems": {}, "version": 1,
                           "options": {"cats": ["R", "R_icon_color"]}}),
        serde_json::json!({"items": [], "pickedItems": {}, "version": 1, "options": {"cats": ["_x"]}}),
        serde_json::json!({"items": [{"name": "  ", "cat": "R"}], "pickedItems": {},
                           "version": 1, "options": {"cats": ["R"]}}),
        // A name of nothing but a non-breaking space: Rust trims it, an
        // ASCII-only regex does not.
        serde_json::json!({"items": [{"name": "\u{00a0}", "cat": "R"}], "pickedItems": {},
                           "version": 1, "options": {"cats": ["R"]}}),
        serde_json::json!({"items": [{"name": "Milk"}], "pickedItems": {}, "version": 1,
                           "options": {"cats": ["R"]}}),
        serde_json::json!({"items": [{"name": "Milk", "cat": "R", "count": "2"}],
                           "pickedItems": {}, "version": 1, "options": {"cats": ["R"]}}),
        serde_json::json!({"items": [], "pickedItems": {}, "version": "7", "options": {"cats": []}}),
        serde_json::json!({"items": [], "pickedItems": {}, "options": {"cats": []}}),
        serde_json::json!({"items": [], "pickedItems": {}, "version": 1}),
        serde_json::json!({"items": {}, "pickedItems": {}, "version": 1, "options": {"cats": []}}),
        serde_json::json!({"items": [], "pickedItems": [], "version": 1, "options": {"cats": []}}),
        serde_json::json!({"items": [1], "pickedItems": {}, "version": 1, "options": {"cats": []}}),
        serde_json::json!({"items": [], "pickedItems": {"Rice": 1}, "version": 1,
                           "options": {"cats": []}}),
    ]
}

#[test]
fn the_mapping_matches_the_bespoke_parse_on_the_captured_shapes() {
    let mapper = RowMapper::compile("shopping/pull_list.response", &response_filter())
        .expect("the shipped filter compiles");
    let compiled = connection();
    let mut divergences = Vec::new();
    for body in captured() {
        if let Err(why) = compare(&mapper, &compiled, &body) {
            divergences.push(why);
        }
    }
    assert!(
        divergences.is_empty(),
        "{} divergence(s) between the sidecar mapping and the parse it replaces. Each must be a \
         bugfunnel entry carrying a ruling, never an allowlist:\n\n{}",
        divergences.len(),
        divergences.join("\n\n")
    );
}

// --------------------------------------------------------------- generators

fn cat_code() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("R".to_string()),
        Just("B".to_string()),
        Just("Ca".to_string()),
        Just("Kleidung".to_string()),
        "[A-Za-z]{1,4}".prop_map(|s| s),
    ]
}

fn cat_entry() -> impl Strategy<Value = String> {
    prop_oneof![
        cat_code(),
        (cat_code(), "[a-z]{1,6}").prop_map(|(c, i)| format!("{c}_{i}")),
        (cat_code(), "[a-z]{1,6}", "[0-9A-F]{6}").prop_map(|(c, i, k)| format!("{c}_{i}_{k}")),
        // Adversarial: an empty code, and more decorations than observed.
        Just("_x".to_string()),
        Just("A_b_c_d".to_string()),
    ]
}

fn item_name() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("Milk".to_string()),
        Just("Brot".to_string()),
        Just("Öl".to_string()),
        Just("  ".to_string()),
        Just("".to_string()),
        // Rust's `str::trim` uses the Unicode White_Space property; a regex
        // `\s` is ASCII-only unless the engine opts in. These three names are
        // where the two definitions can disagree.
        Just("\u{00a0}".to_string()),
        Just("\u{2003}".to_string()),
        Just("\u{3000}".to_string()),
        "[\\PC]{1,8}",
    ]
}

fn count() -> impl Strategy<Value = Json> {
    prop_oneof![
        Just(Json::Null),
        (0i64..5).prop_map(|n| serde_json::json!(n)),
        (-3.0f64..3.0).prop_map(|f| serde_json::json!(f)),
        Just(serde_json::json!("2")),
    ]
}

/// The `(name, cat)` a generated item lands on.
///
/// Drawn from a SMALL hot pool on purpose: `(name, cat)` is this peer's whole
/// identity, so two items sharing one is the duplicate-fold arm — the arm that
/// sums counts and ORs `checked`. Drawing names widely made that arm a rare
/// accident that ~256 default cases missed, leaving it defended only by the
/// captured examples. The wide, adversarial draw stays reachable as the tail.
fn item_key() -> impl Strategy<Value = (String, String)> {
    prop_oneof![
        6 => Just(("Milk".to_string(), "R".to_string())),
        6 => Just(("Brot".to_string(), "B".to_string())),
        6 => Just(("Öl".to_string(), "Ca".to_string())),
        3 => (item_name(), cat_code()),
    ]
}

fn active_item() -> impl Strategy<Value = Json> {
    (item_key(), count(), any::<bool>()).prop_map(|((name, cat), c, with_count)| {
        let mut o = serde_json::Map::new();
        o.insert("name".into(), Json::String(name));
        o.insert("cat".into(), Json::String(cat));
        if with_count {
            o.insert("count".into(), c);
        }
        Json::Object(o)
    })
}

fn picked_entry() -> impl Strategy<Value = (String, Json)> {
    (item_key(), count(), any::<bool>()).prop_map(|((name, cat), c, with_count)| {
        let mut o = serde_json::Map::new();
        o.insert("cat".into(), Json::String(cat));
        if with_count {
            o.insert("count".into(), c);
        }
        (name, Json::Object(o))
    })
}

fn body() -> impl Strategy<Value = Json> {
    (
        prop::collection::vec(active_item(), 0..7),
        prop::collection::vec(picked_entry(), 0..4),
        0i64..50,
        prop::option::of(prop::option::of(0i64..50)),
        prop::collection::vec(cat_entry(), 0..6),
    )
        .prop_map(|(items, picked, version, picked_version, cats)| {
            let mut o = serde_json::Map::new();
            o.insert("items".into(), Json::Array(items));
            o.insert(
                "pickedItems".into(),
                Json::Object(picked.into_iter().collect()),
            );
            o.insert("version".into(), serde_json::json!(version));
            if let Some(v) = picked_version {
                o.insert(
                    "pickedItemsVersion".into(),
                    match v {
                        Some(n) => serde_json::json!(n),
                        None => Json::Null,
                    },
                );
            }
            o.insert("options".into(), serde_json::json!({ "cats": cats }));
            Json::Object(o)
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn the_mapping_matches_the_bespoke_parse_on_generated_responses(body in body()) {
        let mapper = RowMapper::compile("shopping/pull_list.response", &response_filter())
            .expect("the shipped filter compiles");
        let compiled = connection();
        if let Err(why) = compare(&mapper, &compiled, &body) {
            prop_assert!(false, "{why}");
        }
    }
}
