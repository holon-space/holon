//! `inv-no-machine-keychain-access` — a test process never asks the machine
//! for a credential.
//!
//! @pbt oracle internal-consistency — the process's platform-keychain attempt
//!   count is zero (no ref)
//! @pbt covers credential isolation — a harness that resolves the OS store
//!   instead of the injected in-memory one
//! @pbt slips-if-removed a harness binds the platform store, the developer's
//!   login keychain answers with the system authorization dialog, and the run
//!   files fixture secrets into it
//!
//! Capability-free and unbounded on purpose: the subject is what the PROCESS
//! did, not what the SUT holds, so the invariant selects on every slice rather
//! than only where some capability happens to be supplied.

use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvNoMachineKeychainAccess;

impl InvNoMachineKeychainAccess {
    pub const ID: InvariantId = InvariantId("inv-no-machine-keychain-access");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvNoMachineKeychainAccess {
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, _: &S) -> InvariantResult {
        let attempts = holon_secrets::machine_keychain_attempts();
        if attempts == 0 {
            return InvariantResult::Ok;
        }
        InvariantResult::Fail(format!(
            "[inv-no-machine-keychain-access] this test process asked the machine's keychain \
             {attempts} time(s). Every session needs a store handed to it — in-memory for the \
             headless harnesses, the throwaway backend for a driven binary. A run that binds the \
             platform store raises the system authorization dialog and files fixture secrets in \
             the developer's keychain."
        ))
    }
}
