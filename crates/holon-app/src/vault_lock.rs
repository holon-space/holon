//! One writer per vault (Model.md invariant 4, at the process boundary).
//!
//! Every session that opens a vault writes into it: org write-back, the
//! `.loro` snapshots and sidecars, `device.key`, `.holon/*`. Two such sessions
//! each save their whole Loro document, so the last saver drops the other's
//! edits. The first session therefore holds an exclusive `flock` on
//! `{vault}/.holon/writer.lock` for its lifetime, and any other session on the
//! same vault refuses to boot. The OS drops the lock when the holder dies, so
//! a crash leaves nothing to clean up.

use std::fs::File;
use std::fs::OpenOptions;
use std::fs::TryLockError;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;

/// Per-vault Holon state, relative to the vault root.
pub const VAULT_STATE_DIR: &str = ".holon";
/// The writer lock, inside [`VAULT_STATE_DIR`].
pub const WRITER_LOCK_FILE: &str = "writer.lock";

/// Create `vault_root` and seed an empty one with the bundled `index.org`.
///
/// A mobile platform hands the app a data dir, not a vault, so its entry runs
/// this before the boot. [`VaultLock::acquire`] stays strict: a desktop user
/// who names a missing vault has made a mistake the boot must report.
pub fn ensure_vault_dir(vault_root: &Path) -> Result<()> {
    std::fs::create_dir_all(vault_root)
        .with_context(|| format!("creating the vault {}", vault_root.display()))?;
    let empty = std::fs::read_dir(vault_root)
        .with_context(|| format!("listing the vault {}", vault_root.display()))?
        .next()
        .is_none();
    if empty {
        // The Journals page is seeded at boot as blocks: a `Journals.org`
        // here would make a second Journals page.
        let seed = vault_root.join("index.org");
        std::fs::write(&seed, DEFAULT_INDEX_ORG)
            .with_context(|| format!("seeding the vault with {}", seed.display()))?;
    }
    Ok(())
}

const DEFAULT_INDEX_ORG: &str = include_str!("../../../assets/default/index.org");

/// The held writer lock of one vault. Released by [`VaultLock::release`] or,
/// failing that, when the value drops and the file closes.
#[derive(Debug)]
pub struct VaultLock {
    vault_root: PathBuf,
    file: Mutex<Option<File>>,
}

impl VaultLock {
    /// Take the writer lock of `vault_root`, or refuse because another
    /// session holds it.
    pub fn acquire(vault_root: &Path) -> Result<Self> {
        if !vault_root.is_dir() {
            return Err(anyhow!(
                "refusing to start: vault {} does not exist or is not a directory",
                vault_root.display()
            ));
        }
        let dir = vault_root.join(VAULT_STATE_DIR);
        match std::fs::create_dir(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("creating the vault state dir {}", dir.display()));
            }
        }
        let path = dir.join(WRITER_LOCK_FILE);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("opening the vault writer lock {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(anyhow!(
                    "refusing to start: vault {} is held by {}. Quit that instance first.",
                    vault_root.display(),
                    describe_holder(&path)
                ));
            }
            Err(TryLockError::Error(e)) => {
                return Err(e).with_context(|| {
                    format!(
                        "taking the vault writer lock {} failed (a vault on a network file \
                         system cannot be locked)",
                        path.display()
                    )
                });
            }
        }

        // Who holds the lock, for the refusal message only: the `flock` is the
        // truth.
        let holder = serde_json::json!({
            "pid": std::process::id(),
            "binary": std::env::current_exe()
                .context("resolving this binary's path for the vault lock")?
                .display()
                .to_string(),
            "started_at": chrono::Utc::now().to_rfc3339(),
        });
        file.set_len(0)
            .and_then(|()| file.write_all(format!("{holder}\n").as_bytes()))
            .with_context(|| format!("recording the holder in {}", path.display()))?;

        Ok(Self {
            vault_root: vault_root.to_path_buf(),
            file: Mutex::new(Some(file)),
        })
    }

    /// Give the vault to the next writer. Called once the session wrote its
    /// last byte into the vault; a later call does nothing.
    pub fn release(&self) -> Result<()> {
        let Some(file) = self.file.lock().expect("vault lock mutex poisoned").take() else {
            return Ok(());
        };
        file.unlock().with_context(|| {
            format!(
                "releasing the writer lock of vault {}",
                self.vault_root.display()
            )
        })
    }
}

fn describe_holder(path: &Path) -> String {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) => {
            return format!(
                "an instance whose record in {} cannot be read ({e})",
                path.display()
            );
        }
    };
    let recorded = serde_json::from_str::<serde_json::Value>(raw.trim())
        .ok() // ALLOW(ok): an unparseable record is reported verbatim below
        .and_then(|v| {
            Some((
                v.get("pid")?.as_u64()?,
                v.get("binary")?.as_str()?.to_string(),
                v.get("started_at")?.as_str()?.to_string(),
            ))
        });
    match recorded {
        Some((pid, binary, started_at)) => format!("pid {pid} ({binary}, since {started_at})"),
        None => format!(
            "a Holon instance that has not recorded itself yet ({} holds {raw:?})",
            path.display()
        ),
    }
}

/// The vault a session writes, held from before its DB opens until
/// `shutdown_session` releases it. Every composition that boots a session
/// registers one.
#[derive(Debug)]
pub enum SessionVault {
    /// The session is configured without a vault.
    Absent,
    Held(VaultLock),
}

impl SessionVault {
    pub fn acquire(vault_root: Option<&Path>) -> Result<Self> {
        Ok(match vault_root {
            Some(root) => Self::Held(VaultLock::acquire(root)?),
            None => Self::Absent,
        })
    }

    pub fn assert_holds(&self, vault_root: Option<&Path>) {
        let held = match self {
            Self::Absent => None,
            Self::Held(lock) => Some(lock.vault_root.as_path()),
        };
        assert_eq!(
            held, vault_root,
            "the session vault the composition acquired is not the configured one"
        );
    }

    pub fn register(self, injector: &fluxdi::Injector) {
        let vault = fluxdi::Shared::new(self);
        injector.provide::<SessionVault>(fluxdi::Provider::root(move |_| vault.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_vault_refuses_a_second_lock_and_names_the_holder() {
        let vault = tempfile::tempdir().unwrap();
        let held = VaultLock::acquire(vault.path()).unwrap();
        let err = VaultLock::acquire(vault.path()).unwrap_err().to_string();
        assert!(
            err.contains(&format!("pid {}", std::process::id())),
            "the refusal must name the holder's pid: {err}"
        );
        held.release().unwrap();
        VaultLock::acquire(vault.path()).expect("a released vault can be locked again");
    }

    #[test]
    fn dropping_the_holder_frees_the_vault() {
        let vault = tempfile::tempdir().unwrap();
        drop(VaultLock::acquire(vault.path()).unwrap());
        VaultLock::acquire(vault.path()).expect("a dropped holder frees the vault");
    }

    #[test]
    fn a_missing_vault_root_is_refused_without_creating_it() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("mistyped");
        let err = format!("{:#}", VaultLock::acquire(&root).unwrap_err());
        assert!(
            err.contains(&root.display().to_string()),
            "the refusal must name the missing vault: {err}"
        );
        assert!(
            !root.exists(),
            "acquiring a missing vault created {}",
            root.display()
        );
    }

    #[test]
    fn an_unreadable_holder_record_keeps_the_refusal() {
        let vault = tempfile::tempdir().unwrap();
        let _held = VaultLock::acquire(vault.path()).unwrap();
        std::fs::write(
            vault.path().join(VAULT_STATE_DIR).join(WRITER_LOCK_FILE),
            [0xff, 0xfe, 0xfd],
        )
        .unwrap();
        let err = format!("{:#}", VaultLock::acquire(vault.path()).unwrap_err());
        assert!(
            err.contains("refusing to start") && err.contains(&vault.path().display().to_string()),
            "an unreadable holder record must not hide the refusal: {err}"
        );
    }

    #[test]
    fn a_second_release_is_harmless() {
        let vault = tempfile::tempdir().unwrap();
        let held = VaultLock::acquire(vault.path()).unwrap();
        held.release().unwrap();
        held.release()
            .expect("a released lock releases again without effect");
    }

    #[test]
    fn two_vaults_lock_independently() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let _a = VaultLock::acquire(a.path()).unwrap();
        VaultLock::acquire(b.path()).expect("another vault is not held");
    }

    #[derive(Debug, Clone)]
    enum VaultState {
        /// Missing, under `depth` missing parents.
        Absent {
            depth: usize,
        },
        Empty,
        Holding(Vec<(String, Vec<u8>)>),
        /// A file sits where the vault dir belongs.
        FileInTheWay,
    }

    fn any_vault_state() -> impl proptest::strategy::Strategy<Value = VaultState> {
        use proptest::prelude::*;
        prop_oneof![
            (0usize..4).prop_map(|depth| VaultState::Absent { depth }),
            Just(VaultState::Empty),
            prop::collection::btree_map(
                "[a-z]{1,8}(\\.org)?",
                prop::collection::vec(any::<u8>(), 0..64),
                1..4
            )
            .prop_map(|files| VaultState::Holding(files.into_iter().collect())),
            Just(VaultState::FileInTheWay),
        ]
    }

    fn files_under(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                let bytes = std::fs::read(&path).unwrap();
                (path, bytes)
            })
            .collect()
    }

    proptest::proptest! {
        /// Whatever the vault's state, the result is a lockable vault dir:
        /// a missing or empty one holds exactly the bundled `index.org`, an
        /// existing one keeps its bytes, and a second call changes nothing.
        /// A file in the way is an error that leaves the file alone.
        #[test]
        fn ensure_vault_dir_makes_a_lockable_vault(state in any_vault_state()) {
            let temp = tempfile::tempdir().unwrap();
            let vault = match &state {
                VaultState::Absent { depth } => (0..*depth)
                    .fold(temp.path().to_path_buf(), |p, i| p.join(format!("d{i}")))
                    .join("vault"),
                _ => temp.path().join("vault"),
            };
            match &state {
                VaultState::Absent { .. } => {}
                VaultState::Empty => std::fs::create_dir(&vault).unwrap(),
                VaultState::Holding(files) => {
                    std::fs::create_dir(&vault).unwrap();
                    for (name, bytes) in files {
                        std::fs::write(vault.join(name), bytes).unwrap();
                    }
                }
                VaultState::FileInTheWay => std::fs::write(&vault, b"not a vault").unwrap(),
            }

            let result = ensure_vault_dir(&vault);
            if let VaultState::FileInTheWay = state {
                proptest::prop_assert!(result.is_err());
                proptest::prop_assert_eq!(std::fs::read(&vault).unwrap(), b"not a vault".to_vec());
                proptest::prop_assert!(VaultLock::acquire(&vault).is_err());
                return Ok(());
            }
            result.unwrap();
            proptest::prop_assert!(vault.is_dir());
            let after = files_under(&vault);
            let expected = match &state {
                VaultState::Holding(files) => files
                    .iter()
                    .map(|(name, bytes)| (vault.join(name), bytes.clone()))
                    .collect(),
                _ => std::collections::BTreeMap::from([(
                    vault.join("index.org"),
                    DEFAULT_INDEX_ORG.as_bytes().to_vec(),
                )]),
            };
            proptest::prop_assert_eq!(&after, &expected);

            ensure_vault_dir(&vault).unwrap();
            proptest::prop_assert_eq!(files_under(&vault), after);
            VaultLock::acquire(&vault).expect("an ensured vault can be locked");
        }
    }
}
