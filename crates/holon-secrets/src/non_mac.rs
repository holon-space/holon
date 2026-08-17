use crate::KeychainStore;

#[cfg(all(any(unix, windows), not(any(target_os = "ios", target_os = "android"))))]
mod keyring_backend {
    use anyhow::Result;

    use crate::KeychainStore;

    /// Windows Credential Manager / *nix Secret Service, via `keyring`.
    pub struct KeyringStore {
        service: String,
    }

    impl KeyringStore {
        pub fn new(service: impl Into<String>) -> Self {
            Self {
                service: service.into(),
            }
        }

        fn entry(&self, account: &str) -> Result<keyring::Entry> {
            keyring::Entry::new(&self.service, account).map_err(|e| {
                anyhow::anyhow!("open keychain entry {:?}/{account:?}: {e}", self.service)
            })
        }
    }

    impl KeychainStore for KeyringStore {
        fn store(&self, account: &str, secret: &[u8]) -> Result<()> {
            self.entry(account)?.set_secret(secret).map_err(|e| {
                anyhow::anyhow!("store secret for {:?}/{account:?}: {e}", self.service)
            })
        }

        fn load(&self, account: &str) -> Result<Option<Vec<u8>>> {
            match self.entry(account)?.get_secret() {
                Ok(bytes) => Ok(Some(bytes)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(anyhow::anyhow!(
                    "load secret for {:?}/{account:?}: {e}",
                    self.service
                )),
            }
        }

        fn delete(&self, account: &str) -> Result<()> {
            match self.entry(account)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(anyhow::anyhow!(
                    "delete secret for {:?}/{account:?}: {e}",
                    self.service
                )),
            }
        }
    }
}

#[cfg(all(any(unix, windows), not(any(target_os = "ios", target_os = "android"))))]
pub(crate) fn platform_store(service: &str) -> Box<dyn KeychainStore> {
    Box::new(keyring_backend::KeyringStore::new(service))
}

#[cfg(not(all(any(unix, windows), not(any(target_os = "ios", target_os = "android")))))]
pub(crate) fn platform_store(_: &str) -> Box<dyn KeychainStore> {
    #[cfg(target_os = "android")]
    {
        Box::new(crate::UnavailableKeychainStore::android_ndk_context_unwired())
    }
    #[cfg(not(target_os = "android"))]
    {
        Box::new(crate::UnavailableKeychainStore::new())
    }
}
