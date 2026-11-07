//! Boots a keystone case from a restored copy of a finished first boot.
//!
//! A [`BootTemplate`] is one fresh boot of a [`BootKey`], shut down cleanly
//! before the harness aligns the initial focus, and copied. A restore boots the
//! unchanged production session over a copy of that template and then aligns
//! the focus, so it skips only the work the template already holds on disk.
//! [`Obs`] is what a restore must reproduce: a restore that observes anything
//! other than its template's first restore panics.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon_api::EntityUri;
use holon_pbt_core::BootFault;
use holon_pbt_core::EditorLeg;
use holon_pbt_core::Projection;
use holon_pbt_core::capabilities::IntegrationConnectTiming;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutConditions;
use holon_pbt_core::capabilities::SutOrderKeys;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::wiring::Actor;
use holon_pbt_core::wiring::StorageAdapter;
use holon_pbt_core::wiring::SyncAdapter;

use crate::pbt::composed::builder::compose_sut_over_existing;
use crate::pbt::composed::harness::ComposedSut;
use crate::pbt::composed::wide_e2e::JournalsSeed;
use crate::pbt::composed::wide_e2e::WideE2E;
use crate::pbt::composed::wide_e2e::WideHandle;
use crate::pbt::composed::wide_e2e::align_initial_focus;
use crate::pbt::composed::wide_e2e::boot_wide_unaligned;
use crate::pbt::composed::wide_e2e::finish_wide_boot;
use crate::pbt::composed::wide_e2e::set_for_wiring;
use crate::pbt::composed::wide_e2e::wide_seed_files;
use crate::pbt::frontend_slice::components::HeadlessFrontendComponent;
use crate::pbt::frontend_slice::components::StoreSnapshot;
use crate::pbt::frontend_slice::components::keystone_boot_clock;
use crate::pbt::op_write_cap::IdResolver;
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
    read_only_homes: Vec<EntityUri>,
    /// Each variable in [`BOOT_ENV`] with its value in this process.
    env: Vec<Option<String>>,
}

/// The environment variables the first boot reads.
const BOOT_ENV: [&str; 3] = [
    "HOLON_FOLDER_COMPANION_SEED",
    "PBT_MUTABLE_TEXT",
    "HOLON_SOAK_SEED_BLOCKS",
];

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
            read_only_homes: ref_state.read_only.homes().iter().cloned().collect(),
            env: BOOT_ENV
                .iter()
                .map(|name| std::env::var(name).ok())
                .collect(), // ALLOW(ok): an unset variable is a key value
        }
    }

    /// Panics unless a boot of this key can be restored.
    fn assert_restorable(&self) {
        assert!(
            self.boot_fault == BootFault::None && self.timing == IntegrationConnectTiming::Instant,
            "[snapshot boot] key {self} has a boot fault / integration timing; such a boot must \
             run fresh"
        );
    }
}

/// Why a case of `ref_state` cannot boot from a template, or `None` when it
/// can.
pub fn why_not_restorable(ref_state: &ReferenceState) -> Option<String> {
    let key = BootKey::of(ref_state);
    if key.boot_fault != BootFault::None {
        return Some(format!("boot fault {:?}", key.boot_fault));
    }
    if key.timing != IntegrationConnectTiming::Instant {
        return Some(format!("integration timing {:?}", key.timing));
    }
    if !set_for_wiring(&ref_state.harness.wiring).has_projection(Projection::ViewModel) {
        return Some("the wiring boots no session".to_string());
    }
    None
}

impl fmt::Display for BootKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.seed_files.iter().map(|(n, _)| n.as_str()).collect();
        write!(
            f,
            "storage={:?} sync={:?} actors={:?} editor-leg={:?} fault={:?} timing={:?} \
             read-only-homes={:?} env={:?} seed={names:?}",
            self.storage_adapters,
            self.sync_adapters,
            self.actors,
            self.editor_leg,
            self.boot_fault,
            self.timing,
            self.read_only_homes,
            BOOT_ENV.iter().zip(&self.env).collect::<Vec<_>>(),
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

    let loro_store = match (frontend.loro_enabled(), frontend.loro_doc_store()) {
        (true, Some(store)) => Some(store),
        (false, None) => None,
        (enabled, store) => panic!(
            "[snapshot boot] Loro enabled {enabled}, but the session {} a Loro store",
            if store.is_some() { "holds" } else { "lacks" }
        ),
    };
    for (name, scope, store) in loro_store.iter().flat_map(|store| {
        [
            ("loro_tree", holon_loro::DocScope::Global, store),
            ("loro_layout", holon_loro::DocScope::Layout, store),
        ]
    }) {
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

/// A cleanly shut down first boot of one [`BootKey`], with no navigation yet:
/// an aligned focus would make the restore's own align a no-op.
pub struct BootTemplate {
    key: BootKey,
    obs: Obs,
    snapshot: StoreSnapshot,
    peer_ids: BTreeSet<u64>,
}

impl BootTemplate {
    pub fn key(&self) -> &BootKey {
        &self.key
    }

    /// The [`Obs`] of the first restore of the template.
    pub fn obs(&self) -> &Obs {
        &self.obs
    }

    /// The copied store: a test corrupts it to show the restore's checks.
    pub fn snapshot(&self) -> &StoreSnapshot {
        &self.snapshot
    }

    /// A template of `ref_state`'s key that no other case shares.
    pub fn build(ref_state: &ReferenceState) -> Self {
        Self::build_after_boot(ref_state, || {})
    }

    /// [`Self::build`] that runs `after_boot` on the boot's runtime once the
    /// boot finished. Panics on any error the template's boot or shutdown
    /// logged: the restore's scope never sees them.
    pub fn build_after_boot(ref_state: &ReferenceState, after_boot: impl FnOnce()) -> Self {
        let key = BootKey::of(ref_state);
        key.assert_restorable();
        let sut = ComposedSut::<WideE2E>::init_built_by(
            ref_state,
            BTreeSet::new(),
            move |resolver| async move {
                let booted = boot_wide_unaligned(&resolver, ref_state, None).await;
                after_boot();
                booted
            },
        );
        let peer_ids = sut.primary_peer_ids().clone();
        let snapshot = sut.finish_with(|handle| async move {
            let frontend = handle
                .frontend()
                .expect("[snapshot boot] only a session boot has a template")
                .clone();
            drop(handle);
            let loro_enabled = set_for_wiring(&ref_state.harness.wiring)
                .has_projection(Projection::EditorState);
            let snapshot = frontend.shut_down_to_snapshot().await;
            snapshot
                .assert_file_hashes_match(if loro_enabled {
                    holon_api::capability::Consolidator::Upstream
                } else {
                    holon_api::capability::Consolidator::Store
                })
                .await;
            snapshot
        });
        let problems = crate::test_tracing::SpanCollector::global().captured_problems();
        assert!(
            problems.is_empty(),
            "[snapshot boot] the template boot of {key} logged {} error(s) or panic(s): {problems:#?}",
            problems.len()
        );
        let obs = observe(&ComposedSut::<WideE2E>::boot_restored(
            ref_state, &snapshot, &peer_ids,
        ));
        Self {
            key,
            obs,
            snapshot,
            peer_ids,
        }
    }
}

// shortcut: the templates live until the process exits, so their temp dirs
// are never removed; give them a drop at exit if the temp dir fills up.
static TEMPLATES: Mutex<Vec<Arc<BootTemplate>>> = Mutex::new(Vec::new());

/// The template for `ref_state`'s key, built on first use and kept for the
/// rest of the process.
pub fn template_for(ref_state: &ReferenceState) -> Arc<BootTemplate> {
    let key = BootKey::of(ref_state);
    let cached = TEMPLATES
        .lock()
        .expect("[snapshot boot] template cache poisoned")
        .iter()
        .find(|t| t.key == key)
        .cloned();
    if let Some(template) = cached {
        return template;
    }
    // Built outside the lock: a build that panics leaves the cache usable.
    let template = Arc::new(BootTemplate::build(ref_state));
    TEMPLATES
        .lock()
        .expect("[snapshot boot] template cache poisoned")
        .push(template.clone());
    template
}

impl ComposedSut<WideE2E> {
    /// [`init_test`](proptest_state_machine::StateMachineTest::init_test) over
    /// a copy of `template` instead of a fresh vault. Panics unless the case's
    /// key is the template's and the restore observes what the template's
    /// first restore observed.
    pub fn init_restored(template: &BootTemplate, ref_state: &ReferenceState) -> Self {
        let key = BootKey::of(ref_state);
        assert!(
            key == template.key,
            "[snapshot boot] template built for {}, case needs {key}",
            template.key
        );
        key.assert_restorable();
        let sut = Self::boot_restored(ref_state, &template.snapshot, &template.peer_ids);
        assert_obs_eq(
            template.obs(),
            &observe(&sut),
            "the restore differs from its template's first restore",
        );
        sut
    }

    fn boot_restored(
        ref_state: &ReferenceState,
        snapshot: &StoreSnapshot,
        peer_ids: &BTreeSet<u64>,
    ) -> Self {
        let sut = Self::init_built_by(ref_state, peer_ids.clone(), |resolver| async move {
            restore_wide(&resolver, ref_state, snapshot).await
        });
        sut.handle()
            .frontend()
            .expect("a restore boots a session")
            .assert_restore_wrote_nothing();
        sut
    }
}

/// [`boot_and_seed_wide`](crate::pbt::composed::wide_e2e::boot_and_seed_wide)
/// over a copy of `snapshot`.
async fn restore_wide(
    resolver: &IdResolver,
    ref_state: &ReferenceState,
    snapshot: &StoreSnapshot,
) -> (CapMap, WideHandle, BTreeSet<EntityUri>) {
    let set = set_for_wiring(&ref_state.harness.wiring);
    let frontend = Arc::new(
        HeadlessFrontendComponent::new_from_snapshot(
            snapshot,
            Duration::from_millis(300),
            set.has_projection(Projection::EditorState),
            set.wiring.editor_leg,
            keystone_boot_clock(),
            None,
            ref_state.integration.timing,
        )
        .await,
    );
    frontend.set_resolver(resolver.clone());
    let drained = frontend
        .injector()
        .resolve::<holon_orgmode::VaultBacklogDrained>();
    tokio::time::timeout(Duration::from_secs(60), drained.wait_drained())
        .await
        .expect("[snapshot boot] the restored backlog did not end within 60s")
        .expect("[snapshot boot] the restored backlog failed");
    let bundle = compose_sut_over_existing(&set, resolver, frontend).await;
    let (mut caps, handle, scaffold) = finish_wide_boot(bundle, ref_state, true).await;
    align_initial_focus(&mut caps, &handle, ref_state).await;
    (caps, handle, scaffold)
}
