//! This device's network identity, once its key is read.
//!
//! [`ShareIdentity`] holds the only device [`SecretKey`] and the only iroh
//! advertiser of a process. The share backend and the pairing op hold an
//! [`IdentitySlot`] instead, and every network leg needs a `&ShareIdentity`,
//! which only [`IdentitySlot::require`] hands out.

use std::sync::Arc;

use holon_conditions::condition_bus::ConditionBus;
use iroh::PublicKey;
use iroh::SecretKey;

use crate::iroh_advertiser::IrohAdvertiser;

pub struct ShareIdentity {
    public: PublicKey,
    advertiser: Arc<IrohAdvertiser>,
}

impl ShareIdentity {
    pub fn new(key: SecretKey, bus: Arc<ConditionBus>) -> Self {
        Self {
            public: key.public(),
            advertiser: Arc::new(IrohAdvertiser::new_with_key(key).with_degraded_bus(bus)),
        }
    }

    pub fn public(&self) -> PublicKey {
        self.public
    }

    pub fn advertiser(&self) -> &Arc<IrohAdvertiser> {
        &self.advertiser
    }
}

/// Why the slot holds no identity yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IdentityAbsent {
    Loading,
    Corrupt,
    Unavailable { cause: String },
}

impl std::fmt::Display for IdentityAbsent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loading => write!(f, "loading"),
            Self::Corrupt => write!(f, "corrupt: create a new device key"),
            Self::Unavailable { cause } => write!(f, "keychain unavailable: {cause}"),
        }
    }
}

/// The network ops' refusal while the device key is not available.
#[derive(Debug, thiserror::Error)]
#[error("sharing is off: the device key is not available ({0})")]
pub struct SharingOff(pub IdentityAbsent);

enum State {
    Absent(IdentityAbsent),
    Ready(Arc<ShareIdentity>),
}

/// The identity of this process, filled once by the device-key net.
pub struct IdentitySlot {
    state: tokio::sync::watch::Sender<State>,
}

impl IdentitySlot {
    pub fn loading() -> Arc<Self> {
        Arc::new(Self {
            state: tokio::sync::watch::Sender::new(State::Absent(IdentityAbsent::Loading)),
        })
    }

    pub fn with_identity(identity: ShareIdentity) -> Arc<Self> {
        let slot = Self::loading();
        slot.fill(identity);
        slot
    }

    /// Records why there is no identity. A filled slot stays filled.
    pub fn mark_absent(&self, why: IdentityAbsent) {
        self.state.send_modify(|state| {
            if let State::Absent(old) = state {
                *old = why;
            }
        });
    }

    pub fn fill(&self, identity: ShareIdentity) -> Arc<ShareIdentity> {
        let identity = Arc::new(identity);
        self.state.send_modify(|state| {
            assert!(
                matches!(*state, State::Absent(_)),
                "the device identity is filled once per process"
            );
            *state = State::Ready(identity.clone());
        });
        identity
    }

    pub fn ready(&self) -> Option<Arc<ShareIdentity>> {
        match &*self.state.borrow() {
            State::Ready(identity) => Some(identity.clone()),
            State::Absent(_) => None,
        }
    }

    /// The identity, or the one error every network op returns without it.
    pub fn require(&self) -> Result<Arc<ShareIdentity>, SharingOff> {
        match &*self.state.borrow() {
            State::Ready(identity) => Ok(identity.clone()),
            State::Absent(why) => Err(SharingOff(why.clone())),
        }
    }

    /// [`require`](Self::require) once the first load has an outcome.
    pub async fn loaded(&self) -> Result<Arc<ShareIdentity>, SharingOff> {
        self.state
            .subscribe()
            .wait_for(|state| !matches!(state, State::Absent(IdentityAbsent::Loading)))
            .await
            .expect("the slot owns its sender");
        self.require()
    }
}
