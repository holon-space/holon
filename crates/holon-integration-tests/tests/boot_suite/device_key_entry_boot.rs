//! A boot over any device-key entry keeps the vault and its shares usable,
//! keeps sharing off until there is a key, and replaces an unreadable key
//! only on the user's own confirm. A key file left in the storage directory
//! is removed unread and never becomes the identity.
//!
//! @pbt kind harness
//! @pbt covers device-key-entry-boot — every key-entry state before a second
//! boot × legacy key file × shares × edits × confirm × aside account × retry
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone has no reboot
//! transition, so it cannot draw a key-entry state before a boot

use std::collections::HashMap;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon_api::EntityName;
use holon_api::OpOrigin;
use holon_api::Value;
use holon_conditions::ConditionBus;
use holon_conditions::condition_bus::ConditionKind;
use holon_conditions::condition_bus::DEVICE_KEY_SUBJECT;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use holon_loro::device_key_custody::DeviceKeyCustody;
use holon_loro::device_key_custody::aside_stamp;
use holon_loro::device_key_custody::testing::FaultKeychainStore;
use holon_loro::device_key_custody::testing::KeyEntryState;
use holon_loro::device_key_custody::testing::key_entry_state;
use holon_loro::device_key_custody::testing::shared_store;
use holon_loro::share_identity::IdentitySlot;
use holon_loro_wiring::device_key_net;
use holon_loro_wiring::device_key_net::DeviceKeyNet;
use proptest::prelude::*;
use proptest::test_runner::TestCaseError;

const PAGES: usize = 3;
const BOOT_MILLIS: i64 = 1_790_000_000_000;
const SHARING_OFF: &str = "sharing is off: the device key is not available";

/// What the storage directory holds under the legacy key file names before
/// the second boot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LegacyFile {
    None,
    File,
    FileAndTmp,
    /// A link to a key file outside the storage directory.
    Symlink,
    /// A directory in the file's place: removal fails without making the
    /// storage directory read-only, which would also stop the boot's own
    /// writes.
    Unremovable,
}

#[derive(Clone, Debug)]
struct Case {
    entry: KeyEntryState,
    legacy: LegacyFile,
    /// Pages `0..shares` are shared; page `PAGES - 1` never is.
    shares: usize,
    edits: usize,
    confirm: bool,
    aside_taken: bool,
    retry: bool,
}

fn any_legacy_file() -> impl Strategy<Value = LegacyFile> {
    prop_oneof![
        Just(LegacyFile::None),
        Just(LegacyFile::File),
        Just(LegacyFile::FileAndTmp),
        Just(LegacyFile::Symlink),
        Just(LegacyFile::Unremovable),
    ]
}

fn any_case() -> impl Strategy<Value = Case> {
    (
        (key_entry_state(), any_legacy_file()),
        0..PAGES,
        0usize..=3,
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |((entry, legacy), shares, edits, confirm, aside_taken, retry)| Case {
                entry,
                legacy,
                shares,
                edits,
                confirm,
                aside_taken,
                retry,
            },
        )
}

/// The red case first, then one case per entry state, each with the remedy
/// that state offers.
fn static_cases() -> Vec<Case> {
    let case = |entry, shares, confirm, aside_taken, retry| Case {
        entry,
        legacy: LegacyFile::None,
        shares,
        edits: 1,
        confirm,
        aside_taken,
        retry,
    };
    vec![
        Case {
            edits: 0,
            ..case(KeyEntryState::WrongLength(5), 1, false, false, false)
        },
        Case {
            legacy: LegacyFile::FileAndTmp,
            ..case(KeyEntryState::Valid { seed: 7 }, 2, false, false, false)
        },
        Case {
            legacy: LegacyFile::Symlink,
            ..case(KeyEntryState::Absent, 1, false, false, false)
        },
        Case {
            legacy: LegacyFile::File,
            ..case(KeyEntryState::WrongLength(0), 2, true, false, false)
        },
        case(KeyEntryState::WrongLength(64), 1, true, true, false),
        case(KeyEntryState::LoadErr { seed: 11 }, 1, false, false, true),
        case(KeyEntryState::StoreErr, 1, false, false, true),
        case(KeyEntryState::StoreLost, 0, false, false, true),
        Case {
            legacy: LegacyFile::Unremovable,
            ..case(KeyEntryState::Blocking { seed: 13 }, 2, false, false, false)
        },
    ]
}

fn page_id(n: usize) -> String {
    format!("dk-page-{n}")
}

fn block_id(n: usize) -> String {
    format!("block:dk-block-{n}")
}

fn page_file(n: usize) -> (String, String) {
    (
        format!("Dk{n}.org"),
        format!(
            "#+TITLE: Dk{n}\n#+ID: {}\n* Block {n}\n:PROPERTIES:\n:ID: dk-block-{n}\n:END:\n",
            page_id(n)
        ),
    )
}

macro_rules! check {
    ($cond:expr, $($fmt:tt)+) => {
        if !$cond {
            return Err(format!($($fmt)+));
        }
    };
}

async fn until(what: &str, done: impl Fn() -> bool) -> Result<(), String> {
    for _ in 0..3000 {
        if done() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err(format!("timed out waiting for {what}"))
}

async fn share(env: &TestEnvironment, page: usize) -> anyhow::Result<()> {
    let params: HashMap<String, Value> = [
        ("id", format!("block:{}", page_id(page))),
        ("retention", "none".to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Value::String(v)))
    .collect();
    env.execute_operation("tree", "share_subtree", params).await
}

async fn remedy(env: &TestEnvironment, op: &str) -> Result<(), String> {
    env.execute_operation("device", op, HashMap::new())
        .await
        .map_err(|e| format!("{e:#}"))
}

async fn block_rows(env: &TestEnvironment) -> Result<Vec<(String, String)>, String> {
    let ids: Vec<String> = (0..PAGES).map(|n| format!("'{}'", block_id(n))).collect();
    let rows = env
        .query_sql(&format!(
            "SELECT id, content FROM block WHERE id IN ({}) ORDER BY id",
            ids.join(", ")
        ))
        .await
        .map_err(|e| format!("query the page blocks: {e:#}"))?;
    Ok(rows
        .iter()
        .map(|r| {
            (
                format!("{:?}", r.get("id")),
                format!("{:?}", r.get("content")),
            )
        })
        .collect())
}

fn device_conditions(bus: &ConditionBus) -> Vec<ConditionKind> {
    bus.current()
        .into_iter()
        .filter(|c| c.subject == DEVICE_KEY_SUBJECT)
        .map(|c| c.reason)
        .collect()
}

fn count(conditions: &[ConditionKind], pick: fn(&ConditionKind) -> bool) -> usize {
    conditions.iter().filter(|c| pick(c)).count()
}

fn is_corrupt(c: &ConditionKind) -> bool {
    matches!(c, ConditionKind::DeviceKeyCorrupt { .. })
}

fn is_unavailable(c: &ConditionKind) -> bool {
    matches!(c, ConditionKind::DeviceKeyUnavailable { .. })
}

fn is_replaced(c: &ConditionKind) -> bool {
    matches!(c, ConditionKind::DeviceKeyReplaced { .. })
}

/// Any 32 bytes are an ed25519 secret; the pattern differs from every
/// fixture seed's key.
fn planted_key_bytes() -> [u8; 32] {
    std::array::from_fn(|i| (i as u8).wrapping_mul(37).wrapping_add(11))
}

/// Plants the legacy key files and returns the paths that must be gone after
/// the boot.
fn plant_legacy_files(
    legacy: LegacyFile,
    storage_dir: &std::path::Path,
    outside: &std::path::Path,
) -> Result<Vec<std::path::PathBuf>, String> {
    let planted = planted_key_bytes();
    let key_file = storage_dir.join("device.key");
    let tmp_file = storage_dir.join("device.key.tmp");
    let write = |path: &std::path::Path| {
        std::fs::write(path, planted).map_err(|e| format!("plant {}: {e}", path.display()))
    };
    match legacy {
        LegacyFile::None => Ok(vec![]),
        LegacyFile::File => write(&key_file).map(|()| vec![key_file]),
        LegacyFile::FileAndTmp => {
            write(&key_file)?;
            write(&tmp_file)?;
            Ok(vec![key_file, tmp_file])
        }
        LegacyFile::Symlink => {
            write(outside)?;
            std::os::unix::fs::symlink(outside, &key_file)
                .map_err(|e| format!("link {}: {e}", key_file.display()))?;
            Ok(vec![key_file])
        }
        LegacyFile::Unremovable => {
            std::fs::create_dir(&key_file)
                .map_err(|e| format!("plant {}: {e}", key_file.display()))?;
            write(&key_file.join("device.key"))?;
            Ok(vec![])
        }
    }
}

/// Whether any file under `dir` holds `needle`.
fn any_file_holds(dir: &std::path::Path, needle: &[u8]) -> Result<bool, String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("list {}: {e}", dir.display()))? {
        let path = entry
            .map_err(|e| format!("list {}: {e}", dir.display()))?
            .path();
        let kind = std::fs::symlink_metadata(&path)
            .map_err(|e| format!("stat {}: {e}", path.display()))?;
        let holds = if kind.is_dir() {
            any_file_holds(&path, needle)?
        } else if kind.is_file() {
            std::fs::read(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?
                .windows(needle.len())
                .any(|w| w == needle)
        } else {
            false
        };
        if holds {
            return Ok(true);
        }
    }
    Ok(false)
}

fn public_of(bytes: &[u8]) -> Result<iroh::PublicKey, String> {
    let key: [u8; 32] = bytes
        .try_into()
        .map_err(|_| format!("the main entry holds {} bytes, not a key", bytes.len()))?;
    Ok(iroh::SecretKey::from_bytes(&key).public())
}

fn injector(env: &TestEnvironment) -> &fluxdi::Injector {
    env.injector().expect("a started session has an injector")
}

async fn boot_twice(
    rt: Arc<tokio::runtime::Runtime>,
    case: &Case,
    store_out: &Mutex<Option<Arc<FaultKeychainStore>>>,
) -> Result<(), String> {
    let mut builder =
        TestEnvironmentBuilder::new().with_clock(Arc::new(holon_api::TestClock::new(BOOT_MILLIS)));
    for n in 0..PAGES {
        let (file, content) = page_file(n);
        builder = builder.with_org_file(file, content);
    }
    let mut env = builder
        .build(rt)
        .await
        .map_err(|e| format!("first boot: {e:#}"))?;
    let store = shared_store(&env.temp_path().to_string_lossy());
    *store_out.lock().unwrap() = Some(store.clone());
    injector(&env)
        .resolve::<Arc<IdentitySlot>>()
        .loaded()
        .await
        .map_err(|e| format!("the first boot mints a key: {e}"))?;
    env.settle().await;
    for page in 0..case.shares {
        share(&env, page)
            .await
            .map_err(|e| format!("share page {page}: {e:#}"))?;
    }
    env.settle().await;
    let rows_before = block_rows(&env).await?;
    let storage_dir = injector(&env)
        .resolve::<holon_loro_wiring::LoroConfig>()
        .storage_dir
        .clone();
    env.stop_app()
        .await
        .map_err(|e| format!("stop the app: {e:#}"))?;

    let account = DeviceKeyCustody::for_storage_dir(store.clone(), &storage_dir)
        .map_err(|e| format!("custody: {e:#}"))?
        .account()
        .to_string();
    let aside = format!("{account}.unreadable-{}", aside_stamp(BOOT_MILLIS));
    store.set_entry(&account, &case.entry);
    let taken_aside = b"an earlier aside entry".to_vec();
    let corrupt = matches!(case.entry, KeyEntryState::WrongLength(_));
    if case.aside_taken && corrupt {
        // `set_entry` holds only key-entry states, so the aside is stored
        // directly.
        holon_secrets::KeychainStore::store(&*store, &aside, &taken_aside)
            .map_err(|e| format!("seed the aside account: {e:#}"))?;
    }
    let entry_before = store.entry(&account);
    let stores_before = store.store_calls();
    let outside_key_file = env.temp_path().join("elsewhere.key");
    let removable = plant_legacy_files(case.legacy, &storage_dir, &outside_key_file)?;

    // The boot completes for every entry state.
    env.start_app(true)
        .await
        .map_err(|e| format!("second boot: {e:#}"))?;
    let slot = injector(&env).resolve::<Arc<IdentitySlot>>();
    let net = injector(&env).resolve::<Arc<DeviceKeyNet>>();
    let bus = injector(&env).resolve::<Arc<ConditionBus>>();
    let pending = matches!(case.entry, KeyEntryState::Blocking { .. });
    if !pending {
        until("the key load", || {
            net.marked(device_key_net::DEVICE_IDENTITY)
                || net.marked(device_key_net::KEY_CORRUPT)
                || net.marked(device_key_net::KEYCHAIN_UNAVAILABLE)
        })
        .await?;
    }
    env.settle().await;

    // One condition per unreadable state, none for a key or a pending load.
    let unavailable = matches!(
        case.entry,
        KeyEntryState::LoadErr { .. } | KeyEntryState::StoreErr | KeyEntryState::StoreLost
    );
    let conditions = device_conditions(&bus);
    check!(
        (
            count(&conditions, is_corrupt),
            count(&conditions, is_unavailable)
        ) == (usize::from(corrupt), usize::from(unavailable)),
        "(corrupt, unavailable) conditions after the boot: {conditions:?}"
    );

    // The key files are gone unread; a symlink's target is kept.
    let unremovable = case.legacy == LegacyFile::Unremovable;
    for path in &removable {
        check!(
            std::fs::symlink_metadata(path).is_err(),
            "{} exists after the boot",
            path.display()
        );
    }
    check!(
        case.legacy != LegacyFile::Symlink || outside_key_file.exists(),
        "the boot removed the symlink's target"
    );
    check!(
        !unremovable || storage_dir.join("device.key").exists(),
        "the unremovable key file is gone"
    );
    // One disclosure per removed file set, one per set that stays.
    let file_conditions: Vec<ConditionKind> = bus
        .current()
        .into_iter()
        .map(|c| c.reason)
        .filter(|r| {
            matches!(
                r,
                ConditionKind::DeviceKeyFileRemoved { .. }
                    | ConditionKind::DeviceKeyFileNotRemoved { .. }
            )
        })
        .collect();
    let removed_files = match file_conditions.as_slice() {
        [ConditionKind::DeviceKeyFileRemoved { files }] => files.len(),
        _ => 0,
    };
    let not_removed = file_conditions
        .iter()
        .filter(|r| matches!(r, ConditionKind::DeviceKeyFileNotRemoved { .. }))
        .count();
    check!(
        (removed_files, not_removed, file_conditions.len())
            == (
                removable.len(),
                usize::from(unremovable),
                usize::from(!removable.is_empty() || unremovable)
            ),
        "key file conditions {file_conditions:?} for {:?}",
        case.legacy
    );

    // The vault content equals the first boot's.
    let rows_after = block_rows(&env).await?;
    check!(
        rows_after == rows_before,
        "page blocks {rows_after:?} after the second boot, {rows_before:?} before"
    );

    // An edit to a shared block reaches SQL whatever the key state.
    for i in 0..case.edits {
        let content = format!("edit {i}");
        env.update_block_content(&block_id(0), &content)
            .await
            .map_err(|e| format!("edit {i}: {e:#}"))?;
        env.settle().await;
        let rows = block_rows(&env).await?;
        check!(
            rows.iter()
                .any(|(id, c)| id.contains(&block_id(0)) && c.contains(&content)),
            "edit {i} is not in SQL: {rows:?}"
        );
    }

    let has_key = matches!(
        case.entry,
        KeyEntryState::Valid { .. } | KeyEntryState::Absent
    );
    if has_key {
        let main = store
            .entry(&account)
            .ok_or("the main entry is empty after a load with a key")?;
        let identity = slot.ready().ok_or("no identity after a load with a key")?;
        check!(
            identity.public() == public_of(&main)?,
            "the identity is not the main entry's key"
        );
        until("the share network start", || {
            net.marked(device_key_net::STARTED)
        })
        .await?;
    } else {
        // Without a key, a network op returns the one disclosed error.
        let refused = share(&env, PAGES - 1).await;
        check!(
            matches!(&refused, Err(e) if format!("{e:#}").contains(SHARING_OFF)),
            "share_subtree without a key: {refused:?}"
        );
        check!(slot.ready().is_none(), "an identity without a key");
        // Nothing was stored or set aside before a confirm.
        check!(
            store.entry(&account) == entry_before,
            "the main entry changed before a confirm"
        );
        if !matches!(
            case.entry,
            KeyEntryState::StoreErr | KeyEntryState::StoreLost
        ) {
            check!(
                store.store_calls() == stores_before,
                "{} stores before a confirm",
                store.store_calls() - stores_before
            );
        }
    }
    check!(
        case.aside_taken && corrupt || store.entry(&aside).is_none(),
        "an aside account exists before a confirm"
    );

    // An agent cannot replace the device key.
    let stores = store.store_calls();
    let by_agent = env
        .session()
        .execute_operation_with_origin(
            &EntityName::new("device"),
            "create_device_key",
            HashMap::new(),
            OpOrigin::Agent {
                session_id: "probe".into(),
                tool_call_id: "probe".into(),
            },
        )
        .await
        .map(|_| ())
        .map_err(|e| format!("{e:#}"));
    check!(
        matches!(&by_agent, Err(e) if e.contains("runs only on the user's own gesture")),
        "an agent's create_device_key: {by_agent:?}"
    );
    check!(
        store.store_calls() == stores,
        "an agent's create_device_key stored"
    );

    if corrupt && case.confirm {
        let old = entry_before.clone().expect("a corrupt entry holds bytes");
        let confirmed = remedy(&env, "create_device_key").await;
        if case.aside_taken {
            // A taken aside account refuses and changes nothing.
            check!(
                matches!(&confirmed, Err(e) if e.contains(&account) && e.contains(&aside)),
                "confirm over a taken aside account: {confirmed:?}"
            );
            check!(
                store.entry(&account) == Some(old) && store.entry(&aside) == Some(taken_aside),
                "an entry changed after a refused confirm"
            );
            check!(
                count(&device_conditions(&bus), is_corrupt) == 1,
                "the corrupt condition cleared after a refused confirm"
            );
            check!(
                slot.ready().is_none(),
                "an identity after a refused confirm"
            );
        } else {
            // The old bytes are set aside and the new key is the identity.
            confirmed.map_err(|e| format!("confirm: {e}"))?;
            check!(
                store.entry(&aside) == Some(old),
                "the aside account does not hold the old bytes"
            );
            let main = store
                .entry(&account)
                .ok_or("no main entry after a confirm")?;
            let identity = slot.ready().ok_or("no identity after a confirm")?;
            check!(
                identity.public() == public_of(&main)?,
                "the identity is not the new main entry's key"
            );
            let conditions = device_conditions(&bus);
            check!(
                count(&conditions, is_corrupt) == 0 && count(&conditions, is_replaced) == 1,
                "conditions after a confirm: {conditions:?}"
            );
            until("the share network start after a confirm", || {
                net.marked(device_key_net::STARTED)
            })
            .await?;
        }
    }

    if unavailable {
        // An unavailable keychain offers no replacement.
        let refused = remedy(&env, "create_device_key").await;
        check!(
            refused.is_err() && store.entry(&account) == entry_before,
            "create_device_key over an unavailable keychain: {refused:?}"
        );
        if case.retry {
            store.clear_faults();
            remedy(&env, "load_device_key")
                .await
                .map_err(|e| format!("retry over an unavailable keychain: {e}"))?;
            until("the key load after a retry", || {
                net.marked(device_key_net::DEVICE_IDENTITY)
            })
            .await?;
            let identity = slot.ready().ok_or("no identity after a retry")?;
            let main = store.entry(&account).ok_or("no main entry after a retry")?;
            check!(
                identity.public() == public_of(&main)?,
                "the identity is not the main entry's key"
            );
            if let Some(original) = case.entry.key_bytes() {
                check!(
                    main == original.to_vec(),
                    "a retry replaced the original key"
                );
            }
            check!(
                count(&device_conditions(&bus), is_unavailable) == 0,
                "the unavailable condition stays after a retry"
            );
        }
    }

    if pending {
        // A load that waits keeps the session usable; the key comes when
        // it returns.
        check!(
            device_conditions(&bus).is_empty(),
            "conditions while the load waits"
        );
        let refused = share(&env, PAGES - 1).await;
        check!(
            matches!(&refused, Err(e) if format!("{e:#}").contains("loading")),
            "share_subtree while the load waits: {refused:?}"
        );
        store.clear_faults();
        until("the share network start after the load returns", || {
            net.marked(device_key_net::STARTED)
        })
        .await?;
        let identity = slot.ready().ok_or("no identity after the load")?;
        let original = case
            .entry
            .key_bytes()
            .expect("a blocking entry holds a key");
        check!(
            identity.public() == public_of(&original)?,
            "the identity is not the original key"
        );
    }

    if let Some(identity) = slot.ready() {
        // The legacy file never becomes the identity.
        check!(
            identity.public() != iroh::SecretKey::from_bytes(&planted_key_bytes()).public(),
            "the identity is the planted file's key"
        );
        // The key lives only in the keychain.
        let main = store
            .entry(&account)
            .ok_or("an identity without a main entry")?;
        check!(
            !any_file_holds(&storage_dir, &main)?,
            "a file under the storage directory holds the identity's key"
        );
    }

    check!(
        holon_secrets::machine_keychain_attempts() == 0,
        "a test boot reached the machine keychain"
    );
    Ok(())
}

fn check_case(case: &Case) -> Result<(), TestCaseError> {
    let rt = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    );
    let store = Mutex::new(None);
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        rt.block_on(boot_twice(rt.clone(), case, &store))
    }));
    // A load that waits on the latch would keep the runtime from shutting down.
    if let Some(store) = store.lock().unwrap().as_ref() {
        store.clear_faults();
    }
    match outcome {
        Ok(result) => result.map_err(TestCaseError::fail),
        Err(panic) => Err(TestCaseError::fail(format!(
            "panicked: {}",
            panic
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| panic.downcast_ref::<&str>().copied())
                .unwrap_or("<non-string panic>")
        ))),
    }
}

#[test]
fn a_boot_over_any_device_key_entry_keeps_the_vault_and_gates_the_replacement() {
    let mut failures: Vec<String> = static_cases()
        .iter()
        .filter_map(|case| check_case(case).err().map(|e| format!("{case:?}: {e}")))
        .collect();
    if let Err(e) = proptest::test_runner::TestRunner::new(ProptestConfig {
        cases: 6,
        failure_persistence: None,
        ..ProptestConfig::default()
    })
    .run(&any_case(), |case| check_case(&case))
    {
        failures.push(format!("generated: {e}"));
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
