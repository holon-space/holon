//! The device-key net. It reads this device's key off the boot path, starts
//! the share network once there is an identity, and replaces an unreadable
//! key only when the user confirms it.
//!
//! ```text
//! KeyToLoad --LoadKey--> DeviceIdentity | KeyCorrupt | KeychainUnavailable
//! KeychainUnavailable --RetryLoadKey (manual)--> KeyToLoad
//! KeyCorrupt --CreateDeviceKey (manual, state probe)--> DeviceIdentity
//! DeviceIdentity + SharesLoaded --Start--> Started
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use holon_api::Clock;
use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::Value;
use holon_conditions::condition_bus::Condition;
use holon_conditions::condition_bus::ConditionBus;
use holon_conditions::condition_bus::ConditionKey;
use holon_conditions::condition_bus::ConditionKind;
use holon_conditions::condition_bus::DEVICE_KEY_SUBJECT;
use holon_conditions::condition_profile::OpId;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::storage::types::StorageEntity;
use holon_loro::device_key_custody::DeviceKeyCustody;
use holon_loro::device_key_custody::DeviceKeyError;
use holon_loro::device_key_custody::DeviceKeyLoad;
use holon_loro::device_key_custody::aside_stamp;
use holon_loro::device_key_custody::load_device_identity;
use holon_loro::loro_share_backend::LoadedShare;
use holon_loro::loro_share_backend::LoroShareBackend;
use holon_loro::share_identity::IdentityAbsent;
use holon_loro::share_identity::IdentitySlot;
use holon_loro::share_identity::ShareIdentity;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::NetDef;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Reader;
use holon_net::exec::Receiver;
use holon_net::exec::Settled;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;
use holon_net_shell::Shell;
use holon_secrets::DEVICE_KEY_SERVICE;

pub const KEY_TO_LOAD: &str = "KeyToLoad";
pub const DEVICE_IDENTITY: &str = "DeviceIdentity";
pub const KEY_CORRUPT: &str = "KeyCorrupt";
pub const KEYCHAIN_UNAVAILABLE: &str = "KeychainUnavailable";
pub const SHARES_LOADED: &str = "SharesLoaded";
pub const STARTED: &str = "Started";

pub const LOAD_KEY: &str = "LoadKey";
pub const RETRY_LOAD_KEY: &str = "RetryLoadKey";
pub const CREATE_DEVICE_KEY: &str = "CreateDeviceKey";
pub const START: &str = "Start";

fn present(place: &'static str) -> holon_net::exec::Read {
    Arc::new(move |r: &mut Reader<'_>, s: &Subject| r.present(place, s.as_str()).then(Content::new))
}

pub fn net() -> NetDef {
    NetDef::default()
        .place(KEY_TO_LOAD, Class::Process)
        .place(DEVICE_IDENTITY, Class::Process)
        .place(KEY_CORRUPT, Class::Process)
        .place(KEYCHAIN_UNAVAILABLE, Class::Process)
        .place(SHARES_LOADED, Class::Process)
        .place(STARTED, Class::Process)
        .transition(
            TransitionDef::new(
                LOAD_KEY,
                Trigger::Auto,
                present(KEY_TO_LOAD),
                Body::Effectful(Receiver::None),
            )
            .consumes(KEY_TO_LOAD)
            .fact(DEVICE_IDENTITY)
            .fact(KEY_CORRUPT)
            .fact(KEYCHAIN_UNAVAILABLE),
        )
        .transition(
            TransitionDef::new(
                RETRY_LOAD_KEY,
                Trigger::Manual,
                present(KEYCHAIN_UNAVAILABLE),
                Body::pure(|s, _| Ok(vec![Output::new(KEY_TO_LOAD, s, Content::new())])),
            )
            .consumes(KEYCHAIN_UNAVAILABLE)
            .produces(KEY_TO_LOAD),
        )
        .transition(
            TransitionDef::new(
                CREATE_DEVICE_KEY,
                Trigger::Manual,
                present(KEY_CORRUPT),
                Body::Effectful(Receiver::StateProbe),
            )
            .consumes(KEY_CORRUPT)
            .fact(DEVICE_IDENTITY)
            .on_failure(KEY_CORRUPT),
        )
        .transition(
            TransitionDef::new(
                START,
                Trigger::Auto,
                Arc::new(|r: &mut Reader<'_>, s: &Subject| {
                    (r.present(DEVICE_IDENTITY, s.as_str()) && r.present(SHARES_LOADED, s.as_str()))
                        .then(Content::new)
                }),
                Body::Effectful(Receiver::None),
            )
            .reads(DEVICE_IDENTITY)
            .consumes(SHARES_LOADED)
            .fact(STARTED),
        )
}

struct Effects {
    /// The custody, or why the storage directory gave no account.
    custody: Result<Arc<DeviceKeyCustody>, String>,
    slot: Arc<IdentitySlot>,
    bus: Arc<ConditionBus>,
    clock: Arc<dyn Clock>,
    /// The shares the boot loaded, handed to `Start` once.
    loaded: Mutex<Option<(Arc<LoroShareBackend>, Vec<LoadedShare>)>>,
    /// The aside account of a replacement whose main-entry store has an
    /// unknown outcome, for the probe that settles it.
    pending_aside: Mutex<Option<String>>,
}

fn condition_key(kind: &'static str) -> ConditionKey {
    ConditionKey {
        subject: DEVICE_KEY_SUBJECT.to_string(),
        kind,
    }
}

fn token(place: &str, subject: &Subject) -> Vec<Output> {
    vec![Output::new(place, subject, Content::new())]
}

impl Effects {
    fn emit(&self, reason: ConditionKind) {
        self.bus.emit(Condition {
            subject: DEVICE_KEY_SUBJECT.to_string(),
            reason,
        });
    }

    /// Only `LoadKey` meets a custody that could not be built; it marks the
    /// keychain unavailable, which enables no other effect.
    fn custody(&self) -> &Arc<DeviceKeyCustody> {
        self.custody
            .as_ref()
            .expect("only LoadKey runs without a custody")
    }

    async fn blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce(&DeviceKeyCustody) -> T + Send + 'static,
    ) -> T {
        let custody = self.custody().clone();
        tokio::task::spawn_blocking(move || f(&custody))
            .await
            .expect("a device-key keychain call panicked")
    }

    fn identify(&self, key: iroh::SecretKey) {
        self.slot.fill(ShareIdentity::new(key, self.bus.clone()));
    }

    async fn run(&self, request: &EffectRequest) -> EffectResult {
        let subject = &request.subject;
        match &*request.transition {
            LOAD_KEY => self.load_key(subject).await,
            CREATE_DEVICE_KEY if request.probe => self.probe_created(subject).await,
            CREATE_DEVICE_KEY => self.create_device_key(subject).await,
            START => {
                let (backend, loaded) = self
                    .loaded
                    .lock()
                    .unwrap()
                    .take()
                    .expect("Start fires once, after the boot handed over the loaded shares");
                let identity = self
                    .slot
                    .ready()
                    .expect("Start reads DeviceIdentity, which only a filled slot marks");
                backend.start_share_network(&identity, loaded).await;
                EffectResult::Done(token(STARTED, subject))
            }
            other => panic!("the device-key net has no effect for '{other}'"),
        }
    }

    async fn load_key(&self, subject: &Subject) -> EffectResult {
        let load = match &self.custody {
            Ok(_) => self.blocking(load_device_identity).await,
            Err(cause) => DeviceKeyLoad::Unavailable {
                cause: cause.clone(),
            },
        };
        match load {
            DeviceKeyLoad::Loaded(key) | DeviceKeyLoad::Created(key) => {
                self.identify(key);
                self.bus
                    .clear(&condition_key(ConditionKind::DEVICE_KEY_UNAVAILABLE));
                EffectResult::Done(token(DEVICE_IDENTITY, subject))
            }
            // The condition lands before the slot wakes a refused caller.
            DeviceKeyLoad::Corrupt { len } => {
                self.bus
                    .clear(&condition_key(ConditionKind::DEVICE_KEY_UNAVAILABLE));
                self.emit(ConditionKind::DeviceKeyCorrupt {
                    service: DEVICE_KEY_SERVICE.to_string(),
                    account: self.custody().account().to_string(),
                    len,
                });
                self.slot.mark_absent(IdentityAbsent::Corrupt);
                EffectResult::Done(token(KEY_CORRUPT, subject))
            }
            DeviceKeyLoad::Unavailable { cause } => {
                self.emit(ConditionKind::DeviceKeyUnavailable {
                    cause: cause.clone(),
                });
                self.slot.mark_absent(IdentityAbsent::Unavailable { cause });
                EffectResult::Done(token(KEYCHAIN_UNAVAILABLE, subject))
            }
        }
    }

    async fn create_device_key(&self, subject: &Subject) -> EffectResult {
        let stamp = aside_stamp(self.clock.now_millis());
        let main = self.custody().account().to_string();
        let aside = self.custody().aside_account(&stamp);
        match self.blocking(move |c| c.replace_unreadable(&stamp)).await {
            Ok((key, aside)) => {
                self.identify(key);
                self.replaced(aside);
                EffectResult::Done(token(DEVICE_IDENTITY, subject))
            }
            // A keychain fault on the main entry may come after its store
            // landed.
            Err(e)
                if matches!(&e, DeviceKeyError::Keychain { account, .. }
                    | DeviceKeyError::NotPersisted { account } if *account == main) =>
            {
                *self.pending_aside.lock().unwrap() = Some(aside);
                EffectResult::Unknown(Cause::definite(e.to_string()))
            }
            Err(e) => EffectResult::Failed(Cause::definite(e.to_string())),
        }
    }

    /// The goal of `CreateDeviceKey`: the main entry holds a key.
    async fn probe_created(&self, subject: &Subject) -> EffectResult {
        match self.blocking(|c| c.load()).await {
            Ok(Some(key)) => {
                self.identify(key);
                let aside = self
                    .pending_aside
                    .lock()
                    .unwrap()
                    .take()
                    .expect("a probe follows a replacement with an unknown main store");
                self.replaced(aside);
                EffectResult::Done(token(DEVICE_IDENTITY, subject))
            }
            Ok(None) => EffectResult::Failed(Cause::definite(format!(
                "the device key entry {DEVICE_KEY_SERVICE}/{} is empty",
                self.custody().account()
            ))),
            Err(e) => EffectResult::Failed(Cause::definite(e.to_string())),
        }
    }

    fn replaced(&self, aside: String) {
        self.bus
            .clear(&condition_key(ConditionKind::DEVICE_KEY_CORRUPT));
        tracing::warn!(
            "replaced the unreadable device key; the old entry is kept as \
             {DEVICE_KEY_SERVICE}/{aside}"
        );
        self.emit(ConditionKind::DeviceKeyReplaced {
            service: DEVICE_KEY_SERVICE.to_string(),
            aside,
        });
    }
}

/// The running device-key net of one session.
pub struct DeviceKeyNet {
    shell: Shell,
    effects: Arc<Effects>,
    driver: tokio::task::JoinHandle<()>,
}

impl Drop for DeviceKeyNet {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

impl DeviceKeyNet {
    /// Starts the net and its first `LoadKey`, which runs on a blocking
    /// thread: the caller never waits for the keychain. A custody that could
    /// not be built makes the keychain unavailable.
    pub fn start(
        custody: anyhow::Result<DeviceKeyCustody>,
        slot: Arc<IdentitySlot>,
        bus: Arc<ConditionBus>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        let shell = Shell::new(net()).expect("the device-key net is well formed");
        let effects = Arc::new(Effects {
            custody: custody.map(Arc::new).map_err(|e| format!("{e:#}")),
            slot,
            bus,
            clock,
            loaded: Mutex::new(None),
            pending_aside: Mutex::new(None),
        });
        let driver = tokio::spawn(drive(shell.clone(), effects.clone()));
        let net = Self {
            shell,
            effects,
            driver,
        };
        net.put(KEY_TO_LOAD);
        net
    }

    fn put(&self, place: &str) {
        self.shell
            .apply(|exec, now| {
                exec.write(place, DEVICE_KEY_SUBJECT, Content::new(), now)
                    .map(|q| q.requests)
            })
            .unwrap_or_else(|e| panic!("the device-key net takes a {place} token: {e}"));
    }

    /// Hands the shares the boot loaded to `Start`, which runs once there is
    /// an identity.
    pub fn shares_loaded(&self, backend: Arc<LoroShareBackend>, loaded: Vec<LoadedShare>) {
        let previous = self
            .effects
            .loaded
            .lock()
            .unwrap()
            .replace((backend, loaded));
        assert!(previous.is_none(), "the boot loads the shares once");
        self.put(SHARES_LOADED);
    }

    pub fn shell(&self) -> &Shell {
        &self.shell
    }

    pub fn marked(&self, place: &str) -> bool {
        self.shell
            .read(|exec| exec.tokens().token(place, DEVICE_KEY_SUBJECT).is_some())
    }

    /// Fires the manual `transition` and waits for its outcome.
    pub async fn fire_and_settle(&self, transition: &str, occurrence: &str) -> anyhow::Result<()> {
        let occurrence = OccurrenceId::caller(occurrence);
        self.shell
            .fire(transition, DEVICE_KEY_SUBJECT, occurrence.clone())
            .map_err(|e| anyhow::anyhow!("{transition}: {e}"))?;
        match self
            .shell
            .settled(&occurrence)
            .await
            .map_err(|e| anyhow::anyhow!("{transition}: {e}"))?
        {
            Settled::Done => Ok(()),
            Settled::Failed(cause) | Settled::OutcomeUnknown(cause) => {
                anyhow::bail!("{transition}: {}", cause.text)
            }
            Settled::OverChangedInputs { cause, .. } => {
                anyhow::bail!(
                    "{transition}: the inputs changed while it ran: {}",
                    cause.text
                )
            }
        }
    }
}

async fn drive(shell: Shell, effects: Arc<Effects>) {
    loop {
        loop {
            let requests = shell
                .step()
                .unwrap_or_else(|e| panic!("the device-key net stopped: {e}"));
            if requests.is_empty() {
                break;
            }
            for request in requests {
                let result = effects.run(&request).await;
                shell
                    .complete(&request, result)
                    .unwrap_or_else(|e| panic!("the device-key net stopped: {e}"));
            }
        }
        shell.fired().await;
    }
}

const OPS: [(OpId, &str, &str, &str); 2] = [
    (
        OpId::CreateDeviceKey,
        CREATE_DEVICE_KEY,
        "Create a new device key",
        "Replace the unreadable device key with a new one. This device gets a new peer \
         identity, so paired devices and share rosters must pair or enroll it again. The old \
         entry is kept in the keychain.",
    ),
    (
        OpId::LoadDeviceKey,
        RETRY_LOAD_KEY,
        "Read the device key again",
        "Read the device key from the keychain again.",
    ),
];

fn descriptor(op: OpId, display_name: &str, description: &str) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: op.entity().into(),
        entity_short_name: op.entity().to_string(),
        id_column: String::new(),
        name: op.op().to_string(),
        display_name: display_name.to_string(),
        description: description.to_string(),
        required_params: vec![],
        optional_params: vec![],
        affected_fields: vec![],
        param_mappings: vec![],
        target_scope: holon_api::TargetScope::Global,
        // Reached from the condition's remedy slot, never from a block menu.
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::External,
        },
        boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
        trigger: None,
        bound_params: Default::default(),
        marking_delta: holon_api::marking::MarkingDelta::Static { kinds: vec![] },
        guard: holon_api::pattern::OpGuard::None,
        arcs: holon_api::arcs::TransitionArcs::Declared {
            reads: vec![],
            emits: vec![],
        },
    }
}

/// Serves the device-key remedies [`OPS`].
pub struct DeviceKeyRemedyProvider {
    net: Arc<DeviceKeyNet>,
    issued: std::sync::atomic::AtomicU64,
}

impl DeviceKeyRemedyProvider {
    pub fn new(net: Arc<DeviceKeyNet>) -> Self {
        Self {
            net,
            issued: Default::default(),
        }
    }
}

const GESTURE_ONLY: &[&str] = &[OpId::CreateDeviceKey.op()];

#[async_trait::async_trait]
impl OperationProvider for DeviceKeyRemedyProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        OPS.iter()
            .map(|(op, _, name, description)| descriptor(*op, name, description))
            .collect()
    }

    fn user_gesture_only_ops(&self) -> &'static [&'static str] {
        GESTURE_ONLY
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        _: StorageEntity,
    ) -> holon_core::Result<OperationResult> {
        let (op, transition, _, _) = OPS
            .iter()
            .find(|(op, ..)| *entity_name == EntityName::new(op.entity()) && op_name == op.op())
            .ok_or_else(|| {
                format!("DeviceKeyRemedyProvider: does not serve '{entity_name}::{op_name}'")
            })?;
        let n = self
            .issued
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let occurrence = format!("{}:{n}", op.op());
        self.net
            .fire_and_settle(transition, &occurrence)
            .await
            .map_err(|e| format!("{}: {e:#}", op.op()))?;
        Ok(OperationResult::declared_irreversible(
            Vec::new(),
            "the device key lives in the OS keychain, outside the undo log",
        )
        .with_response(Value::Object(HashMap::from([(
            "settled".to_string(),
            Value::String(occurrence),
        )]))))
    }
}

#[cfg(test)]
mod tests {
    use holon_loro::share_identity::SharingOff;

    use super::*;

    /// Example, not a property: the input is one class, a storage directory
    /// with no account, and its cause text is opaque to the net.
    #[tokio::test]
    async fn a_custody_that_cannot_be_built_makes_the_keychain_unavailable() {
        let slot = IdentitySlot::loading();
        let bus = Arc::new(ConditionBus::new());
        let _net = DeviceKeyNet::start(
            Err(anyhow::anyhow!(
                "canonicalize the storage directory /nowhere"
            )),
            slot.clone(),
            bus.clone(),
            Arc::new(holon_api::TestClock::new(0)),
        );
        let refusal = tokio::time::timeout(std::time::Duration::from_secs(10), slot.loaded())
            .await
            .expect("the first load settles")
            .err()
            .expect("no identity without a custody");
        assert!(
            matches!(&refusal, SharingOff(IdentityAbsent::Unavailable { cause }) if cause.contains("/nowhere")),
            "{refusal}"
        );
        let unavailable: Vec<_> = bus
            .current()
            .into_iter()
            .filter(|c| matches!(c.reason, ConditionKind::DeviceKeyUnavailable { .. }))
            .collect();
        assert_eq!(unavailable.len(), 1, "{unavailable:?}");
    }
}
