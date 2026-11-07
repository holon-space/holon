//! Boots a keystone case from a restored copy of a finished first boot.
//!
//! A [`BootTemplate`] is one fresh boot of a [`BootKey`], shut down cleanly and
//! copied. A restore boots the unchanged production session over a copy of
//! that template, so it skips only the work the template already holds on
//! disk. [`Obs`] is what a restore must reproduce: a restore that observes
//! anything other than its template's fresh boot panics.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use holon_pbt_core::BootFault;
use holon_pbt_core::EditorLeg;
use holon_pbt_core::capabilities::IntegrationConnectTiming;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutConditions;
use holon_pbt_core::capabilities::SutOrderKeys;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::wiring::Actor;
use holon_pbt_core::wiring::StorageAdapter;
use holon_pbt_core::wiring::SyncAdapter;

use crate::pbt::composed::harness::ComposedSut;
use crate::pbt::composed::wide_e2e::JournalsSeed;
use crate::pbt::composed::wide_e2e::WideE2E;
use crate::pbt::composed::wide_e2e::wide_seed_files;
use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;
use crate::pbt::reference_state::ReferenceState;

/// Everything in a [`ReferenceState`] the first boot reads. Two states with
/// equal keys boot to equal [`Obs`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BootKey {
    storage_adapters: Vec<StorageAdapter>,
    sync_adapters: Vec<SyncAdapter>,
    actors: Vec<Actor>,
    editor_leg: EditorLeg,
    seed_files: Vec<(String, String)>,
    boot_fault: BootFault,
    timing: IntegrationConnectTiming,
}

impl BootKey {
    pub fn of(ref_state: &ReferenceState) -> Self {
        let wiring = &ref_state.harness.wiring;
        Self {
            storage_adapters: wiring.storage_adapters.iter().copied().collect(),
            sync_adapters: wiring.sync_adapters.iter().copied().collect(),
            actors: wiring.actors.iter().copied().collect(),
            editor_leg: wiring.editor_leg,
            seed_files: wide_seed_files(ref_state, JournalsSeed::File)
                .into_iter()
                .map(|(name, body)| (name.to_string(), body.to_string()))
                .collect(),
            boot_fault: ref_state.boot_fault.clone(),
            timing: ref_state.integration.timing,
        }
    }
}

impl fmt::Display for BootKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.seed_files.iter().map(|(n, _)| n.as_str()).collect();
        write!(
            f,
            "storage={:?} sync={:?} actors={:?} editor-leg={:?} fault={:?} timing={:?} seed={names:?}",
            self.storage_adapters,
            self.sync_adapters,
            self.actors,
            self.editor_leg,
            self.boot_fault,
            self.timing,
        )
    }
}

/// What a booted session shows, with every path inside its vault written as
/// [`VAULT`]. One map per field, keyed by the item the field holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Obs {
    fields: BTreeMap<&'static str, BTreeMap<String, String>>,
}

/// The vault root as [`Obs`] prints it.
pub const VAULT: &str = "<vault>";

impl Obs {
    /// One line per item that differs, each naming its field and item.
    pub fn diff(&self, actual: &Obs) -> Vec<String> {
        let mut lines = Vec::new();
        let empty = BTreeMap::new();
        let names: std::collections::BTreeSet<&&str> =
            self.fields.keys().chain(actual.fields.keys()).collect();
        for name in names {
            let expected = self.fields.get(*name).unwrap_or(&empty);
            let got = actual.fields.get(*name).unwrap_or(&empty);
            let items: std::collections::BTreeSet<&String> =
                expected.keys().chain(got.keys()).collect();
            for item in items {
                match (expected.get(item), got.get(item)) {
                    (Some(e), Some(g)) if e == g => {}
                    (e, g) => lines.push(format!(
                        "{name}[{item}]: expected {}, got {}",
                        e.map_or("<absent>".to_string(), |v| format!("{v:?}")),
                        g.map_or("<absent>".to_string(), |v| format!("{v:?}")),
                    )),
                }
            }
        }
        lines
    }
}

/// Panics with [`Obs::diff`] unless the two are equal.
pub fn assert_obs_eq(expected: &Obs, actual: &Obs, context: &str) {
    let diff = expected.diff(actual);
    assert!(
        diff.is_empty(),
        "[snapshot boot] {context}: {} item(s) differ:\n{}",
        diff.len(),
        diff.join("\n")
    );
}

/// The [`Obs`] of a booted keystone SUT.
pub fn observe(sut: &ComposedSut<WideE2E>) -> Obs {
    let frontend = sut
        .handle()
        .frontend()
        .expect("[snapshot boot] only a session boot has an Obs");
    sut.runtime().block_on(observe_frontend(frontend))
}

async fn observe_frontend(frontend: &HeadlessFrontendComponent) -> Obs {
    let root = frontend.org_root().to_string_lossy().into_owned();
    let unrooted = |text: String| text.replace(&root, VAULT);
    let mut fields: BTreeMap<&'static str, BTreeMap<String, String>> = BTreeMap::new();

    fields.insert(
        "vault_file",
        frontend
            .vault_file_contents()
            .await
            .into_iter()
            .map(|(path, text)| {
                (
                    unrooted(path.to_string_lossy().into_owned()),
                    unrooted(text),
                )
            })
            .collect(),
    );

    let order: BTreeMap<String, String> = frontend
        .live_block_order_keys()
        .await
        .into_iter()
        .map(|(id, key)| (id.to_string(), key))
        .collect();
    fields.insert(
        "block_row",
        frontend
            .block_raw_snapshot()
            .await
            .into_iter()
            .map(|block| {
                let id = block.id.to_string();
                let row = format!(
                    "parent={} order={:?} content={:?} content_type={:?} source_language={:?} properties={}",
                    block.parent_id,
                    order.get(&id),
                    block.content,
                    block.content_type,
                    block.source_language,
                    sorted_keys(
                        serde_json::to_value(&block.properties)
                            .expect("[snapshot boot] block properties serialize")
                    ),
                );
                (id, unrooted(row))
            })
            .collect(),
    );

    let store = frontend
        .loro_doc_store()
        .expect("[snapshot boot] a session boot keeps its Loro store");
    for (name, scope) in [
        ("loro_tree", holon_loro::DocScope::Global),
        ("loro_layout", holon_loro::DocScope::Layout),
    ] {
        let doc = store
            .get_doc(scope)
            .await
            .unwrap_or_else(|e| panic!("[snapshot boot] open the {name} doc: {e}"));
        let value = doc
            .with_read(|doc| Ok(doc.get_deep_value()))
            .unwrap_or_else(|e| panic!("[snapshot boot] read the {name} doc: {e}"));
        let loro::LoroValue::Map(roots) = value else {
            panic!("[snapshot boot] the {name} doc's deep value is not a map: {value:?}");
        };
        let mut peers = Vec::new();
        fields.insert(
            name,
            roots
                .iter()
                .map(|(container, value)| {
                    let json = loro_json(value, &mut peers);
                    (
                        container.clone(),
                        unrooted(serde_json::to_string_pretty(&json).expect("json prints")),
                    )
                })
                .collect(),
        );
    }

    fields.insert(
        "render",
        BTreeMap::from([(
            "root".to_string(),
            unrooted(format!(
                "{:#?}",
                frontend.widget_tree_snapshot_fresh().await
            )),
        )]),
    );
    fields.insert(
        "condition",
        frontend
            .conditions_now()
            .await
            .into_iter()
            .enumerate()
            .map(|(ix, c)| (format!("{ix}:{}", c.kind), unrooted(format!("{c:?}"))))
            .collect(),
    );
    fields.insert(
        "nav",
        frontend
            .live_focus_root_rows()
            .await
            .into_iter()
            .map(|(region, root)| (region, unrooted(root)))
            .collect(),
    );
    Obs { fields }
}

/// Loro map keys whose values [`Obs`] does not compare, each with its reason.
const EXCLUDED_LORO_KEYS: &[(&str, &str)] = &[
    ("created_at", "the wall-clock time of the write"),
    ("updated_at", "the wall-clock time of the write"),
];

/// `value` with every object's keys in sorted order, so a map that came from a
/// `HashMap` prints the same way each time.
fn sorted_keys(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<_> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k, sorted_keys(v)))
                    .collect(),
            )
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(sorted_keys).collect())
        }
        other => other,
    }
}

/// A Loro deep value as JSON. Each boot writes as a random Loro peer, so every
/// `<counter>@<peer>` names its peer by order of first appearance; a string
/// that holds a JSON object has its keys sorted.
fn loro_json(value: &loro::LoroValue, peers: &mut Vec<String>) -> serde_json::Value {
    use loro::LoroValue;
    match value {
        LoroValue::Null => serde_json::Value::Null,
        LoroValue::Bool(b) => serde_json::Value::Bool(*b),
        LoroValue::Double(d) => serde_json::json!(d),
        LoroValue::I64(i) => serde_json::json!(i),
        LoroValue::Binary(bytes) => serde_json::json!(bytes.to_vec()),
        LoroValue::String(text) => serde_json::Value::String(loro_text(text, peers)),
        LoroValue::Container(id) => serde_json::Value::String(loro_text(&id.to_string(), peers)),
        LoroValue::List(items) => {
            serde_json::Value::Array(items.iter().map(|v| loro_json(v, peers)).collect())
        }
        LoroValue::Map(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            serde_json::Value::Object(
                entries
                    .into_iter()
                    .map(|(key, v)| {
                        let shown = match EXCLUDED_LORO_KEYS.iter().find(|(k, _)| k == key) {
                            Some((_, reason)) => {
                                serde_json::Value::String(format!("<excluded: {reason}>"))
                            }
                            None => loro_json(v, peers),
                        };
                        (key.clone(), shown)
                    })
                    .collect(),
            )
        }
    }
}

fn loro_text(text: &str, peers: &mut Vec<String>) -> String {
    if text.starts_with('{')
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(text)
    {
        return sorted_keys(json).to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        let (head, tail) = rest.split_at(at);
        out.push_str(head);
        let digits = tail[1..].bytes().take_while(u8::is_ascii_digit).count();
        let counter_before = head.ends_with(|c: char| c.is_ascii_digit());
        if digits == 0 || !counter_before {
            out.push('@');
            rest = &tail[1..];
            continue;
        }
        let peer = &tail[1..=digits];
        let index = match peers.iter().position(|p| p == peer) {
            Some(index) => index,
            None => {
                peers.push(peer.to_string());
                peers.len() - 1
            }
        };
        out.push_str(&format!("@peer{index}"));
        rest = &tail[1 + digits..];
    }
    out.push_str(rest);
    out
}

/// A finished, cleanly shut down first boot of one [`BootKey`].
pub struct BootTemplate {
    key: BootKey,
    obs: Obs,
}

impl BootTemplate {
    pub fn key(&self) -> &BootKey {
        &self.key
    }

    /// The [`Obs`] of the fresh boot, read before its shutdown.
    pub fn obs(&self) -> &Obs {
        &self.obs
    }
}

/// The template for `ref_state`'s key, built on first use and kept for the
/// rest of the process.
pub fn template_for(ref_state: &ReferenceState) -> Arc<BootTemplate> {
    let key = BootKey::of(ref_state);
    todo!("[snapshot boot] build the template for {key}")
}

impl ComposedSut<WideE2E> {
    /// [`proptest_state_machine::StateMachineTest::init_test`] over a copy of
    /// `template` instead of a fresh vault.
    pub fn init_restored(template: &BootTemplate, ref_state: &ReferenceState) -> Self {
        let _ = (template, ref_state);
        todo!("[snapshot boot] restore a boot from its template")
    }
}
