//! Custody of this device's network identity key in the OS keychain.
//!
//! The key authenticates every iroh endpoint of this device. It lives in the
//! keychain service [`DEVICE_KEY_SERVICE`] under one account per canonical
//! storage directory, so two vaults on one machine keep two identities and no
//! key byte is ever written to a file. Nothing here logs, formats or debugs
//! key bytes.

use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use holon_secrets::DEVICE_KEY_SERVICE;
use holon_secrets::KeychainStore;
use iroh::SecretKey;

/// The keychain account of the storage directory `canonical_storage_dir`.
pub fn device_key_account(canonical_storage_dir: &Path) -> String {
    let digest = blake3::hash(canonical_storage_dir.as_os_str().as_encoded_bytes());
    format!("device/{}", &digest.to_hex()[..16])
}

/// Why the device key could not be read or written. Every message names the
/// service and the account, never a key byte.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DeviceKeyError {
    #[error("the device key entry {DEVICE_KEY_SERVICE}/{account} holds {len} bytes, not 32")]
    WrongLength { account: String, len: usize },
    #[error(
        "the device key entry {DEVICE_KEY_SERVICE}/{account} already exists; refusing to \
         replace it"
    )]
    AlreadyPresent { account: String },
    #[error(
        "the device key entry {DEVICE_KEY_SERVICE}/{account} did not read back as it was stored"
    )]
    NotPersisted { account: String },
    #[error(
        "the device key entry {DEVICE_KEY_SERVICE}/{account} is not unreadable any more; \
         nothing was replaced"
    )]
    NotUnreadable { account: String },
    #[error(
        "cannot set the unreadable device key {DEVICE_KEY_SERVICE}/{account} aside: \
         {DEVICE_KEY_SERVICE}/{aside} already exists"
    )]
    AsideTaken { account: String, aside: String },
    #[error("the keychain failed to {step} {DEVICE_KEY_SERVICE}/{account}: {cause}")]
    Keychain {
        account: String,
        step: &'static str,
        cause: String,
    },
}

/// The device key's custody: one keychain account in [`DEVICE_KEY_SERVICE`].
///
/// No `Debug`: nothing about custody belongs in a log line.
pub struct DeviceKeyCustody {
    keychain: Arc<dyn KeychainStore>,
    account: String,
}

impl DeviceKeyCustody {
    pub fn new(keychain: Arc<dyn KeychainStore>, account: String) -> Self {
        Self { keychain, account }
    }

    /// Custody for the storage directory `storage_dir`, which is created first
    /// so its canonical spelling, and therefore the account, is the same on
    /// every boot.
    pub fn for_storage_dir(
        keychain: Arc<dyn KeychainStore>,
        storage_dir: &Path,
    ) -> anyhow::Result<Self> {
        use anyhow::Context;
        std::fs::create_dir_all(storage_dir)
            .with_context(|| format!("create the storage directory {}", storage_dir.display()))?;
        let canonical = std::fs::canonicalize(storage_dir).with_context(|| {
            format!(
                "canonicalize the storage directory {}",
                storage_dir.display()
            )
        })?;
        Ok(Self::new(keychain, device_key_account(&canonical)))
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    /// The stored key. `Ok(None)` iff the keychain holds no entry.
    pub fn load(&self) -> Result<Option<SecretKey>, DeviceKeyError> {
        let Some(bytes) = self.load_raw(&self.account)? else {
            return Ok(None);
        };
        let seed: [u8; 32] =
            bytes
                .as_slice()
                .try_into()
                .map_err(|_| DeviceKeyError::WrongLength {
                    account: self.account.clone(),
                    len: bytes.len(),
                })?;
        Ok(Some(SecretKey::from_bytes(&seed)))
    }

    /// Mints and stores a new key into an account that holds none.
    pub fn create(&self) -> Result<SecretKey, DeviceKeyError> {
        if self.load_raw(&self.account)?.is_some() {
            return Err(DeviceKeyError::AlreadyPresent {
                account: self.account.clone(),
            });
        }
        let key = SecretKey::generate();
        self.store_verified(&self.account, &key.to_bytes())?;
        Ok(key)
    }

    /// Replaces an entry that is not a key with a new key, after copying the
    /// old bytes to the free account [`aside_account`](Self::aside_account)
    /// for `stamp`. Returns the new key and the aside account.
    pub fn replace_unreadable(&self, stamp: &str) -> Result<(SecretKey, String), DeviceKeyError> {
        let old = match self.load_raw(&self.account)? {
            Some(bytes) if bytes.len() != 32 => bytes,
            _ => {
                return Err(DeviceKeyError::NotUnreadable {
                    account: self.account.clone(),
                });
            }
        };
        let aside = self.aside_account(stamp);
        if self.load_raw(&aside)?.is_some() {
            return Err(DeviceKeyError::AsideTaken {
                account: self.account.clone(),
                aside,
            });
        }
        self.store_verified(&aside, &old)?;
        let key = SecretKey::generate();
        self.store_verified(&self.account, &key.to_bytes())?;
        Ok((key, aside))
    }

    /// The account an unreadable entry is kept under when it is replaced at
    /// `stamp`.
    pub fn aside_account(&self, stamp: &str) -> String {
        format!("{}.unreadable-{stamp}", self.account)
    }

    fn load_raw(&self, account: &str) -> Result<Option<Vec<u8>>, DeviceKeyError> {
        self.keychain
            .load(account)
            .map_err(|e| keychain_error(account, "load", e))
    }

    fn store_verified(&self, account: &str, bytes: &[u8]) -> Result<(), DeviceKeyError> {
        self.keychain
            .store(account, bytes)
            .map_err(|e| keychain_error(account, "store", e))?;
        match self.load_raw(account)? {
            Some(stored) if stored == bytes => Ok(()),
            _ => Err(DeviceKeyError::NotPersisted {
                account: account.to_string(),
            }),
        }
    }
}

/// The stamp of an aside account for a replacement at `now_millis` since the
/// Unix epoch, in UTC to the millisecond.
pub fn aside_stamp(now_millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(now_millis)
        .unwrap_or_else(|| panic!("{now_millis} ms is not a representable instant"))
        .format("%Y%m%dT%H%M%S%.3fZ")
        .to_string()
}

fn keychain_error(account: &str, step: &'static str, cause: anyhow::Error) -> DeviceKeyError {
    DeviceKeyError::Keychain {
        account: account.to_string(),
        step,
        cause: format!("{cause:#}"),
    }
}

/// What reading the device key at start gave.
pub enum DeviceKeyLoad {
    Loaded(SecretKey),
    /// The account held no entry; a new key was stored and read back.
    Created(SecretKey),
    /// The entry holds `len` bytes, which is not a key.
    Corrupt {
        len: usize,
    },
    /// The keychain failed; `cause` names service, account and step.
    Unavailable {
        cause: String,
    },
}

impl std::fmt::Debug for DeviceKeyLoad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loaded(key) => write!(f, "Loaded(node {})", key.public()),
            Self::Created(key) => write!(f, "Created(node {})", key.public()),
            Self::Corrupt { len } => write!(f, "Corrupt {{ len: {len} }}"),
            Self::Unavailable { cause } => write!(f, "Unavailable {{ cause: {cause:?} }}"),
        }
    }
}

/// Reads the device key, minting one when the account holds none. Never
/// stores after a failed read, and never panics.
pub fn load_device_identity(custody: &DeviceKeyCustody) -> DeviceKeyLoad {
    match custody.load() {
        Ok(Some(key)) => DeviceKeyLoad::Loaded(key),
        Ok(None) => match custody.create() {
            Ok(key) => DeviceKeyLoad::Created(key),
            Err(e) => DeviceKeyLoad::Unavailable {
                cause: e.to_string(),
            },
        },
        Err(DeviceKeyError::WrongLength { len, .. }) => DeviceKeyLoad::Corrupt { len },
        Err(e) => DeviceKeyLoad::Unavailable {
            cause: e.to_string(),
        },
    }
}

/// File names a storage directory held the device key under.
pub const LEGACY_KEY_FILES: [&str; 2] = ["device.key", "device.key.tmp"];

/// What [`remove_legacy_key_files`] did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LegacyKeyFiles {
    pub removed: Vec<PathBuf>,
    pub not_removed: Vec<(PathBuf, std::io::ErrorKind)>,
}

/// Removes the device key files from `storage_dir` without reading them. A
/// symlink is removed itself, never its target.
pub fn remove_legacy_key_files(storage_dir: &Path) -> LegacyKeyFiles {
    let mut outcome = LegacyKeyFiles::default();
    for name in LEGACY_KEY_FILES {
        let path = storage_dir.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => outcome.not_removed.push((path, e.kind())),
            Ok(_) => match std::fs::remove_file(&path) {
                Ok(()) => outcome.removed.push(path),
                Err(e) => outcome.not_removed.push((path, e.kind())),
            },
        }
    }
    outcome
}

/// Test doubles and the shared generator of device-key entry states.
#[cfg(any(test, feature = "test-helpers"))]
pub mod testing {
    use std::collections::HashMap;
    use std::sync::Condvar;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;

    use super::*;

    /// The device-key entry before a boot, with the fault the keychain shows.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum KeyEntryState {
        Absent,
        /// A key derived from `seed`, so a failure report names the seed and
        /// not the key.
        Valid {
            seed: u64,
        },
        WrongLength(usize),
        /// Every load fails; a valid key from `seed` sits behind the fault.
        LoadErr {
            seed: u64,
        },
        /// No entry, and every store fails.
        StoreErr,
        /// No entry, and a store reports success but keeps nothing.
        StoreLost,
        /// A valid key from `seed`, and every load waits until the latch opens.
        Blocking {
            seed: u64,
        },
    }

    impl KeyEntryState {
        /// The key bytes the entry holds behind its fault, if it holds a key.
        pub fn key_bytes(&self) -> Option<[u8; 32]> {
            match self {
                Self::Valid { seed } | Self::LoadErr { seed } | Self::Blocking { seed } => {
                    Some(seed_key(*seed))
                }
                _ => None,
            }
        }
    }

    pub fn seed_key(seed: u64) -> [u8; 32] {
        *blake3::hash(&seed.to_le_bytes()).as_bytes()
    }

    /// Every entry state, wrong lengths over `0..=64` without 32.
    pub fn key_entry_state() -> impl proptest::strategy::Strategy<Value = KeyEntryState> {
        use proptest::prelude::*;
        prop_oneof![
            Just(KeyEntryState::Absent),
            any::<u64>().prop_map(|seed| KeyEntryState::Valid { seed }),
            (0usize..=64)
                .prop_filter("32 bytes is a key", |n| *n != 32)
                .prop_map(KeyEntryState::WrongLength),
            any::<u64>().prop_map(|seed| KeyEntryState::LoadErr { seed }),
            Just(KeyEntryState::StoreErr),
            Just(KeyEntryState::StoreLost),
            any::<u64>().prop_map(|seed| KeyEntryState::Blocking { seed }),
        ]
    }

    #[derive(Default)]
    struct Faults {
        load_fails: bool,
        store_fails: bool,
        store_lost: bool,
        blocked: bool,
    }

    /// An in-memory keychain with scripted faults that counts its `store`
    /// calls.
    #[derive(Default)]
    pub struct FaultKeychainStore {
        entries: Mutex<HashMap<String, Vec<u8>>>,
        faults: Mutex<Faults>,
        unblocked: Condvar,
        stores: AtomicUsize,
        loads: AtomicUsize,
    }

    impl FaultKeychainStore {
        /// Puts `state` into `account` and arms its fault.
        pub fn set_entry(&self, account: &str, state: &KeyEntryState) {
            let mut entries = self.entries.lock().unwrap();
            entries.remove(account);
            match state {
                KeyEntryState::WrongLength(n) => {
                    entries.insert(account.to_string(), vec![0xA5; *n]);
                }
                other => {
                    if let Some(bytes) = other.key_bytes() {
                        entries.insert(account.to_string(), bytes.to_vec());
                    }
                }
            }
            *self.faults.lock().unwrap() = Faults {
                load_fails: matches!(state, KeyEntryState::LoadErr { .. }),
                store_fails: matches!(state, KeyEntryState::StoreErr),
                store_lost: matches!(state, KeyEntryState::StoreLost),
                blocked: matches!(state, KeyEntryState::Blocking { .. }),
            };
        }

        /// Ends every fault and releases every waiting load.
        pub fn clear_faults(&self) {
            *self.faults.lock().unwrap() = Faults::default();
            self.unblocked.notify_all();
        }

        pub fn entry(&self, account: &str) -> Option<Vec<u8>> {
            self.entries.lock().unwrap().get(account).cloned()
        }

        pub fn accounts(&self) -> Vec<String> {
            self.entries.lock().unwrap().keys().cloned().collect()
        }

        pub fn store_calls(&self) -> usize {
            self.stores.load(Ordering::SeqCst)
        }

        pub fn load_calls(&self) -> usize {
            self.loads.load(Ordering::SeqCst)
        }
    }

    impl KeychainStore for FaultKeychainStore {
        fn reaches_machine_keychain(&self) -> bool {
            false
        }

        fn store(&self, account: &str, secret: &[u8]) -> anyhow::Result<()> {
            self.stores.fetch_add(1, Ordering::SeqCst);
            let faults = self.faults.lock().unwrap();
            anyhow::ensure!(!faults.store_fails, "scripted store failure");
            if !faults.store_lost {
                self.entries
                    .lock()
                    .unwrap()
                    .insert(account.to_string(), secret.to_vec());
            }
            Ok(())
        }

        fn load(&self, account: &str) -> anyhow::Result<Option<Vec<u8>>> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            let mut faults = self.faults.lock().unwrap();
            while faults.blocked {
                faults = self.unblocked.wait(faults).unwrap();
            }
            anyhow::ensure!(!faults.load_fails, "scripted load failure");
            Ok(self.entries.lock().unwrap().get(account).cloned())
        }

        fn delete(&self, account: &str) -> anyhow::Result<()> {
            self.entries.lock().unwrap().remove(account);
            Ok(())
        }
    }

    /// The process-wide store of `namespace`: two calls with one namespace
    /// share the entries, as two boots share the OS keychain.
    pub fn shared_store(namespace: &str) -> Arc<FaultKeychainStore> {
        type Stores = Mutex<HashMap<String, Arc<FaultKeychainStore>>>;
        static STORES: OnceLock<Stores> = OnceLock::new();
        STORES
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .entry(namespace.to_string())
            .or_default()
            .clone()
    }

    /// The key of `namespace`'s shared store, minted on first use, for tests
    /// that rebuild a backend as a restart does.
    pub fn stable_test_key(namespace: &Path) -> SecretKey {
        let custody = DeviceKeyCustody::new(
            shared_store(&namespace.to_string_lossy()),
            "device/test".to_string(),
        );
        match load_device_identity(&custody) {
            DeviceKeyLoad::Loaded(key) | DeviceKeyLoad::Created(key) => key,
            other => panic!("the test device key is not available: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::testing::FaultKeychainStore;
    use super::testing::KeyEntryState;
    use super::testing::key_entry_state;
    use super::*;

    const ACCOUNT: &str = "device/0123456789abcdef";

    fn custody(store: &Arc<FaultKeychainStore>) -> DeviceKeyCustody {
        DeviceKeyCustody::new(store.clone(), ACCOUNT.to_string())
    }

    /// One boot's read, run off the test thread with the latch opened, so a
    /// Blocking entry reads like a keychain dialog that the user answers.
    fn boot(store: &Arc<FaultKeychainStore>, state: &KeyEntryState) -> DeviceKeyLoad {
        if matches!(state, KeyEntryState::Blocking { .. }) {
            let reader = {
                let store = store.clone();
                std::thread::spawn(move || load_device_identity(&custody(&store)))
            };
            while store.load_calls() == 0 {
                std::thread::yield_now();
            }
            store.clear_faults();
            return reader.join().unwrap();
        }
        load_device_identity(&custody(store))
    }

    fn hex_of(bytes: &[u8]) -> String {
        hex::encode(bytes)
    }

    /// Custody over one entry state and a boot count. The first boot meets
    /// the scripted fault; later boots meet the keychain with the fault
    /// cleared, as after a fixed keychain.
    fn check(state: KeyEntryState, boots: usize) -> Result<(), TestCaseError> {
        let store = Arc::new(FaultKeychainStore::default());
        store.set_entry(ACCOUNT, &state);
        let mut secrets_in_play: Vec<Vec<u8>> =
            state.key_bytes().into_iter().map(Vec::from).collect();
        let first = boot(&store, &state);
        let first_debug = format!("{first:?}");

        match (&state, &first) {
            (
                KeyEntryState::Valid { .. } | KeyEntryState::Blocking { .. },
                DeviceKeyLoad::Loaded(key),
            ) => {
                // A stored key loads unchanged and nothing is stored.
                prop_assert_eq!(key.to_bytes(), state.key_bytes().unwrap());
                prop_assert_eq!(store.store_calls(), 0);
            }
            (KeyEntryState::Absent, DeviceKeyLoad::Created(key)) => {
                // An absent entry gets a new key, stored in the entry.
                prop_assert_eq!(store.entry(ACCOUNT), Some(key.to_bytes().to_vec()));
                secrets_in_play.push(key.to_bytes().to_vec());
            }
            (KeyEntryState::WrongLength(n), DeviceKeyLoad::Corrupt { len }) => {
                // A wrong-length entry stays as it is and nothing is stored.
                prop_assert_eq!(n, len);
                prop_assert_eq!(store.store_calls(), 0);
                prop_assert_eq!(store.entry(ACCOUNT).map(|b| b.len()), Some(*n));
            }
            (KeyEntryState::LoadErr { .. }, DeviceKeyLoad::Unavailable { cause }) => {
                // A load fault stores nothing and its cause names the entry.
                prop_assert_eq!(store.store_calls(), 0);
                prop_assert!(cause.contains(ACCOUNT), "{}", cause);
                prop_assert!(cause.contains(DEVICE_KEY_SERVICE), "{}", cause);
            }
            (
                KeyEntryState::StoreErr | KeyEntryState::StoreLost,
                DeviceKeyLoad::Unavailable { .. },
            ) => {
                // A failed store leaves no entry.
                prop_assert_eq!(store.entry(ACCOUNT), None);
            }
            (state, outcome) => {
                return Err(TestCaseError::fail(format!("{state:?} gave {outcome:?}")));
            }
        }

        // Once a key is in the entry, every later boot reads the same key.
        store.clear_faults();
        let mut stable: Option<[u8; 32]> = match &first {
            DeviceKeyLoad::Loaded(k) | DeviceKeyLoad::Created(k) => Some(k.to_bytes()),
            _ => state.key_bytes(),
        };
        let mut texts = vec![first_debug];
        for _ in 1..boots {
            let next = load_device_identity(&custody(&store));
            texts.push(format!("{next:?}"));
            match (&next, stable) {
                (DeviceKeyLoad::Loaded(k), Some(expected)) => {
                    prop_assert_eq!(k.to_bytes(), expected)
                }
                (DeviceKeyLoad::Created(k), None) => {
                    secrets_in_play.push(k.to_bytes().to_vec());
                    stable = Some(k.to_bytes());
                }
                (DeviceKeyLoad::Corrupt { .. }, None) => {
                    prop_assert!(matches!(state, KeyEntryState::WrongLength(_)));
                }
                (other, _) => {
                    return Err(TestCaseError::fail(format!(
                        "{state:?}: later boot gave {other:?}"
                    )));
                }
            }
        }

        // No message and no Debug output carries a key byte string.
        if let KeyEntryState::WrongLength(_) = state {
            let e = custody(&store).load().unwrap_err();
            texts.push(e.to_string());
            texts.push(format!("{e:?}"));
        }
        for text in &texts {
            for secret in &secrets_in_play {
                prop_assert!(!text.contains(&hex_of(secret)), "key hex in {:?}", text);
            }
        }
        Ok(())
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 256,
            ..ProptestConfig::default()
        })]

        #[test]
        fn device_key_custody_properties(state in key_entry_state(), boots in 1usize..=4) {
            check(state, boots)?;
        }
    }

    /// Fixed cases first: first and second launch, a 5-byte entry, a load
    /// error, and a Blocking entry, each over four boots.
    #[test]
    fn device_key_custody_static_examples() {
        for state in [
            KeyEntryState::Absent,
            KeyEntryState::Valid { seed: 7 },
            KeyEntryState::WrongLength(5),
            KeyEntryState::LoadErr { seed: 7 },
            KeyEntryState::Blocking { seed: 7 },
        ] {
            check(state.clone(), 4).unwrap_or_else(|e| panic!("{state:?}: {e}"));
        }
    }

    /// The aside step: the old bytes are kept byte-equal under a free aside
    /// account and the new key reads back; a taken aside account or a crash
    /// after the aside step never yields `Created`.
    #[test]
    fn replacing_an_unreadable_entry_keeps_it_aside() {
        proptest!(ProptestConfig::with_cases(128), |(len in (0usize..=64).prop_filter("key", |n| *n != 32), taken: bool)| {
            let store = Arc::new(FaultKeychainStore::default());
            store.set_entry(ACCOUNT, &KeyEntryState::WrongLength(len));
            let c = custody(&store);
            let aside = c.aside_account("20261124T000000.000Z");
            if taken {
                store.store(&aside, b"other").unwrap();
                let stores = store.store_calls();
                let e = c.replace_unreadable("20261124T000000.000Z").unwrap_err();
                prop_assert_eq!(e, DeviceKeyError::AsideTaken { account: ACCOUNT.into(), aside: aside.clone() });
                prop_assert_eq!(store.store_calls(), stores);
                prop_assert_eq!(store.entry(&aside), Some(b"other".to_vec()));
                prop_assert_eq!(store.entry(ACCOUNT).map(|b| b.len()), Some(len));
                return Ok(());
            }
            let (key, got_aside) = c.replace_unreadable("20261124T000000.000Z").unwrap();
            prop_assert_eq!(&got_aside, &aside);
            prop_assert_eq!(store.entry(&aside), Some(vec![0xA5; len]));
            prop_assert_eq!(store.entry(ACCOUNT), Some(key.to_bytes().to_vec()));
            prop_assert!(matches!(load_device_identity(&c), DeviceKeyLoad::Loaded(k) if k.to_bytes() == key.to_bytes()));
            prop_assert_eq!(c.replace_unreadable("20261124T000001.000Z").unwrap_err(), DeviceKeyError::NotUnreadable { account: ACCOUNT.into() });
        });
        // A crash after the aside step leaves the main entry unreadable: the
        // next read is Corrupt again, never Created.
        let store = Arc::new(FaultKeychainStore::default());
        store.set_entry(ACCOUNT, &KeyEntryState::WrongLength(5));
        store
            .store(&custody(&store).aside_account("s"), &[0xA5; 5])
            .unwrap();
        assert!(matches!(
            load_device_identity(&custody(&store)),
            DeviceKeyLoad::Corrupt { len: 5 }
        ));
    }

    #[test]
    fn legacy_key_files_go_unread_and_only_once() {
        proptest!(ProptestConfig::with_cases(64), |(key in any::<bool>(), tmp in any::<bool>(), link in any::<bool>())| {
            let dir = tempfile::TempDir::new().unwrap();
            let target = dir.path().join("elsewhere");
            std::fs::write(&target, [1u8; 32]).unwrap();
            let mut planted = Vec::new();
            for (present, name) in [(key, "device.key"), (tmp, "device.key.tmp")] {
                if !present { continue; }
                let path = dir.path().join(name);
                if link {
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(&target, &path).unwrap();
                    #[cfg(not(unix))]
                    std::fs::write(&path, [1u8; 32]).unwrap();
                } else {
                    std::fs::write(&path, [1u8; 32]).unwrap();
                }
                planted.push(path);
            }
            let outcome = remove_legacy_key_files(dir.path());
            prop_assert_eq!(&outcome.removed, &planted);
            prop_assert!(outcome.not_removed.is_empty());
            for path in &planted {
                prop_assert!(std::fs::symlink_metadata(path).is_err());
            }
            prop_assert!(target.exists(), "a symlink's target is kept");
            prop_assert_eq!(remove_legacy_key_files(dir.path()), LegacyKeyFiles::default());
        });
    }
}
