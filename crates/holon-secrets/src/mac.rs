use anyhow::Result;

use crate::KeychainStore;

/// macOS Keychain Services backend.
pub struct MacKeychainStore {
    service: String,
}

impl MacKeychainStore {
    /// `errSecItemNotFound` (Security.framework). Hardcoded to avoid taking a
    /// direct dependency on `security-framework-sys`; the value is part of the
    /// stable macOS ABI.
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

    pub fn new(service: impl Into<String>) -> Self {
        Self {
            service: service.into(),
        }
    }
}

impl KeychainStore for MacKeychainStore {
    fn store(&self, account: &str, secret: &[u8]) -> Result<()> {
        use anyhow::Context;
        security_framework::passwords::set_generic_password(&self.service, account, secret)
            .with_context(|| {
                format!(
                    "store secret for {:?}/{account:?} in macOS keychain",
                    self.service
                )
            })
    }

    fn load(&self, account: &str) -> Result<Option<Vec<u8>>> {
        match security_framework::passwords::get_generic_password(&self.service, account) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.code() == Self::ERR_SEC_ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(anyhow::anyhow!(
                "load secret for {:?}/{account:?} from macOS keychain: {e}",
                self.service
            )),
        }
    }

    fn delete(&self, account: &str) -> Result<()> {
        match security_framework::passwords::delete_generic_password(&self.service, account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == Self::ERR_SEC_ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(anyhow::anyhow!(
                "delete secret for {:?}/{account:?} from macOS keychain: {e}",
                self.service
            )),
        }
    }
}
