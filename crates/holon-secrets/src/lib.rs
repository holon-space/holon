//! @c4 component
//! @c4 layer Core
//! Pattern: Adapter
//!
//! OS-keychain storage for Holon's secret material.
//!
//! Two callers share this seam: the owner-identity seed (ADR 0028 C1/D1) and
//! the OAuth2 credentials an integration sidecar references. A secret filed
//! here is identified by a *service* (fixed per store) and an *account*.
//!
//! - [`KeychainStore`] — the trait every backend implements.
//! - [`platform_keychain`] — the backend for the running platform.
//! - [`UnavailableKeychainStore`] — the **fail-loud** stand-in for platforms
//!   with no wired backend. Every call returns a clear `Err` naming the
//!   platform and the missing precondition; it never silently drops the secret
//!   to disk or memory.
//! - [`InMemoryKeychainStore`] — a test double (never used in production).
//!
//! Secrets are opaque `&[u8]` here. Nothing in this crate logs the secret
//! material.

use anyhow::Result;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(not(target_os = "macos"))]
mod non_mac;

/// Storage seam for secret material. Implementations MUST NOT log the secret.
pub trait KeychainStore: Send + Sync {
    /// Store (or overwrite) the secret for `account`.
    fn store(&self, account: &str, secret: &[u8]) -> Result<()>;
    /// Load the secret for `account`. `Ok(None)` iff no entry exists;
    /// any other backend failure is a loud `Err` (never coerced to `None`).
    fn load(&self, account: &str) -> Result<Option<Vec<u8>>>;
    /// Remove the secret for `account`. Absence is not an error.
    fn delete(&self, account: &str) -> Result<()>;
}

/// Fail-loud stand-in for platforms whose keychain is not wired. Every
/// operation errors, naming the platform and the unmet precondition — the
/// secret is NEVER written in the clear instead. Callers surface the error so
/// the user sees a degraded, non-secret-leaking state.
pub struct UnavailableKeychainStore {
    platform: &'static str,
    precondition: &'static str,
}

impl UnavailableKeychainStore {
    pub fn new() -> Self {
        Self {
            platform: std::env::consts::OS,
            precondition: "no keychain backend is compiled in for this platform",
        }
    }

    /// Android has a keychain backend upstream (`keyring`'s
    /// `android-native-keyring-store`), but it reads the app's JNI handle from
    /// `ndk_context::android_context()`, which no Holon frontend publishes.
    pub fn android_ndk_context_unwired() -> Self {
        Self {
            platform: "android",
            precondition: "ndk_context::android_context() is never initialized by the host app",
        }
    }

    fn refuse<T>(&self, op: &str) -> Result<T> {
        anyhow::bail!(
            "keychain is not implemented on {} yet (op: {op}): {}; refusing to \
             keep the secret in the clear instead",
            self.platform,
            self.precondition
        )
    }
}

impl Default for UnavailableKeychainStore {
    fn default() -> Self {
        Self::new()
    }
}

impl KeychainStore for UnavailableKeychainStore {
    fn store(&self, _: &str, _: &[u8]) -> Result<()> {
        self.refuse("store")
    }
    fn load(&self, _: &str) -> Result<Option<Vec<u8>>> {
        self.refuse("load")
    }
    fn delete(&self, _: &str) -> Result<()> {
        self.refuse("delete")
    }
}

/// Keychain service holding the `${VAR}` secrets an integration sidecar
/// references.
///
/// One service for all of them, with the variable name as the account: a
/// sidecar names `${TODOIST_API_KEY}`, so the variable IS the identity, and a
/// per-provider service would make the same token two entries once two
/// sidecars reference it.
pub const INTEGRATION_SECRET_SERVICE: &str = "holon-integrations";

/// The account name a `${VAR}` reference is filed under.
///
/// Lowercase with `.` folded to `_`, so `${TODOIST_API_KEY}` and the
/// `todoist.api_key` preference address ONE entry. This is the canonical
/// definition; `holon_frontend::integration_vars::normalize_var_name`
/// delegates here so the two cannot drift into addressing different accounts.
pub fn secret_account(var: &str) -> String {
    var.to_ascii_lowercase().replace('.', "_")
}

/// The backend for the current platform, filing every secret under `service`.
pub fn platform_keychain(service: &str) -> Box<dyn KeychainStore> {
    #[cfg(target_os = "macos")]
    {
        Box::new(mac::MacKeychainStore::new(service))
    }
    #[cfg(not(target_os = "macos"))]
    {
        non_mac::platform_store(service)
    }
}

/// Select the in-memory backend for a session. Fixture use only.
pub const BACKEND_ENV: &str = "HOLON_SECRETS_BACKEND";

/// The acknowledgement that lets the in-memory backend run over a config
/// directory that is not obviously throwaway.
pub const BACKEND_UNSAFE_ENV: &str = "HOLON_SECRETS_BACKEND_ALLOW_ANY_CONFIG_DIR";

/// The exact value [`BACKEND_UNSAFE_ENV`] must hold. A specific sentence rather
/// than "1": nobody sets this by habit or by copying a shell line they did not
/// read.
pub const BACKEND_UNSAFE_ACK: &str = "i-know-secrets-will-not-persist";

/// The backend a boot must use, and what it must say about it.
pub struct SelectedBackend {
    pub store: Box<dyn KeychainStore>,
    /// The words a degraded-mode banner shows. `None` for the platform
    /// keychain, which is the undegraded case and needs no banner.
    pub disclosure: Option<String>,
}

/// The backend for this process: the platform keychain, or the in-memory one
/// when [`BACKEND_ENV`] asks for it AND the request is admissible.
///
/// The in-memory backend exists so a session driving a RUNNING binary — a
/// dogfood pass, an end-to-end fixture — can exercise the keychain half of the
/// credential path. Before it existed there was no safe way to do that: the
/// only injectable store was reachable in-process, so anything driving the
/// built app either skipped those flows or wrote into the developer's real
/// login keychain.
///
/// Two things keep it out of a real install. It is admitted only when
/// `config_dir` is under the system temp directory or the caller sets
/// [`BACKEND_UNSAFE_ENV`] to [`BACKEND_UNSAFE_ACK`]; and when it IS admitted
/// the caller gets a `disclosure` it must show, because "your credentials are
/// not being saved" is precisely the fact a user must not have to infer.
///
/// An inadmissible or misspelled request is an `Err`, never a quiet fall back
/// to the platform keychain: a fixture that believed it was isolated and was
/// silently writing to the login keychain is the outcome this whole seam
/// exists to prevent.
pub fn backend_from_env(service: &str, config_dir: &std::path::Path) -> Result<SelectedBackend> {
    select_backend(
        service,
        config_dir,
        std::env::var(BACKEND_ENV).ok().as_deref(),
        std::env::var(BACKEND_UNSAFE_ENV).ok().as_deref(),
    )
}

/// The directories a throwaway session may keep its config in, resolved.
///
/// Three roots rather than one, because a machine spells its temp directory
/// more than one way and the rule is about WHERE a directory is. On macOS
/// `std::env::temp_dir()` answers the per-user `/var/folders/…/T/` form,
/// `/tmp` is a symlink to `/private/tmp`, and neither is a path prefix of the
/// other — so a single unresolved comparison refuses two of the three.
fn throwaway_roots() -> Vec<std::path::PathBuf> {
    let mut roots = vec![
        resolve_existing_prefix(&std::env::temp_dir()),
        resolve_existing_prefix(std::path::Path::new("/tmp")),
        resolve_existing_prefix(std::path::Path::new("/private/tmp")),
    ];
    roots.sort();
    roots.dedup();
    roots
}

/// `path` with its deepest EXISTING ancestor canonicalised and the rest
/// re-appended.
///
/// Plain `canonicalize` fails on a path that does not exist yet, and a config
/// directory the boot is about to create is exactly that — judging it by
/// whether it exists would make the rule depend on whether this is a first
/// run.
fn resolve_existing_prefix(path: &std::path::Path) -> std::path::PathBuf {
    let mut suffix: Vec<std::ffi::OsString> = Vec::new();
    let mut cursor = path;
    loop {
        if let Ok(resolved) = cursor.canonicalize() {
            let mut out = resolved;
            out.extend(suffix.iter().rev());
            return out;
        }
        match (cursor.parent(), cursor.file_name()) {
            (Some(parent), Some(name)) => {
                suffix.push(name.to_os_string());
                cursor = parent;
            }
            // Nothing on the way up resolves: judge the path as written rather
            // than inventing one.
            _ => return path.to_path_buf(),
        }
    }
}

/// Whether `config_dir` sits under a directory the machine treats as scratch.
///
/// `Path::starts_with` compares whole components, so `/tmpfoo` is not under
/// `/tmp` — that is the property, not an accident of string prefixes.
fn is_throwaway(config_dir: &std::path::Path) -> bool {
    let resolved = resolve_existing_prefix(config_dir);
    throwaway_roots()
        .iter()
        .any(|root| resolved.starts_with(root))
}

/// The rule [`backend_from_env`] applies, with the environment passed in.
///
/// Separate so the rule is testable without mutating process-global state —
/// `set_var` is unsafe and would make these cases order-dependent on each
/// other.
pub fn select_backend(
    service: &str,
    config_dir: &std::path::Path,
    requested: Option<&str>,
    acknowledgement: Option<&str>,
) -> Result<SelectedBackend> {
    match requested.unwrap_or_default().trim() {
        "" | "platform" => Ok(SelectedBackend {
            store: platform_keychain(service),
            disclosure: None,
        }),
        "memory" => {
            let acknowledged = acknowledgement == Some(BACKEND_UNSAFE_ACK);
            let throwaway = is_throwaway(config_dir);
            anyhow::ensure!(
                throwaway || acknowledged,
                "{BACKEND_ENV}=memory keeps every secret in RAM and loses it on exit, so it is \
                 admitted only for a throwaway session. The config directory is '{}' (resolved: \
                 '{}'), which is under none of {:?}. Point the session at a temp config \
                 directory, or set {BACKEND_UNSAFE_ENV}={BACKEND_UNSAFE_ACK} to say you meant it.",
                config_dir.display(),
                resolve_existing_prefix(config_dir).display(),
                throwaway_roots()
                    .iter()
                    .map(|r| r.display().to_string())
                    .collect::<Vec<_>>()
            );
            Ok(SelectedBackend {
                store: Box::new(InMemoryKeychainStore::new()),
                disclosure: Some(format!(
                    "Secrets are held in memory for this session ({BACKEND_ENV}=memory). Nothing \
                     you type into a credential field is saved, and every stored secret is gone \
                     when Holon exits. Unset {BACKEND_ENV} to use the system keychain."
                )),
            })
        }
        other => anyhow::bail!(
            "{BACKEND_ENV}={other:?} names no secret backend. Use 'platform' (the system \
             keychain, the default) or 'memory' (a throwaway session that saves nothing). \
             Refusing rather than guessing: guessing 'platform' here would write credentials to \
             the login keychain of a session that asked not to."
        ),
    }
}

/// In-memory keychain for tests ONLY. Never wired into production: it provides
/// no at-rest protection.
#[derive(Default)]
pub struct InMemoryKeychainStore {
    entries: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

impl InMemoryKeychainStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl KeychainStore for InMemoryKeychainStore {
    fn store(&self, account: &str, secret: &[u8]) -> Result<()> {
        self.entries
            .lock()
            .unwrap()
            .insert(account.to_string(), secret.to_vec());
        Ok(())
    }
    fn load(&self, account: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.entries.lock().unwrap().get(account).cloned())
    }
    fn delete(&self, account: &str) -> Result<()> {
        self.entries.lock().unwrap().remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_round_trips() {
        let kc = InMemoryKeychainStore::new();
        assert!(kc.load("owner").unwrap().is_none());
        kc.store("owner", b"secret-bytes").unwrap();
        assert_eq!(kc.load("owner").unwrap().unwrap(), b"secret-bytes");
        kc.delete("owner").unwrap();
        assert!(kc.load("owner").unwrap().is_none());
    }

    #[test]
    fn unavailable_store_fails_loud_never_silently_drops() {
        let kc = UnavailableKeychainStore::new();
        let secret_material = "DEADBEEF-owner-seed-material";
        let err = kc.store("owner", secret_material.as_bytes()).unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("not implemented"));
        assert!(msg.contains("refusing"));
        // The secret MATERIAL must never appear in the error text.
        assert!(!msg.contains(secret_material));
        assert!(!msg.contains("DEADBEEF"));
        assert!(kc.load("owner").is_err());
        assert!(kc.delete("owner").is_err());
    }

    /// A throwaway config directory is the ordinary way a fixture gets the
    /// in-memory backend: no acknowledgement, and the session is told what to
    /// put on screen.
    #[test]
    fn a_temp_config_dir_may_hold_secrets_in_memory_and_must_say_so() {
        let dir = std::env::temp_dir().join("holon-secrets-selection-case");
        let selected = select_backend(INTEGRATION_SECRET_SERVICE, &dir, Some("memory"), None)
            .expect("a throwaway config dir admits the in-memory backend");

        selected.store.store("todoist_api_key", b"fixture").unwrap();
        assert_eq!(
            selected.store.load("todoist_api_key").unwrap().unwrap(),
            b"fixture",
            "the backend must actually work, or it exercises nothing"
        );
        let disclosure = selected
            .disclosure
            .expect("a session that saves no secret must say so — that is the whole condition");
        assert!(
            disclosure.contains("memory") && disclosure.contains("gone when Holon exits"),
            "the banner must say both that secrets are in memory and that they do not survive; \
             got {disclosure:?}"
        );
    }

    /// Every spelling of "somewhere throwaway" must be admitted, because the
    /// rule is about WHERE the directory is and a path is not a string.
    ///
    /// `std::env::temp_dir()` on macOS returns the UNRESOLVED per-user
    /// `/var/folders/…/T/` form, so a `starts_with` against it refused `/tmp`,
    /// refused `/private/tmp` (what `/tmp` actually is), and refused the
    /// canonicalised `/private/var/folders/…` form of its own answer. The
    /// visible cost was that `just live-verify`, whose default config dir is
    /// `/tmp/holon-live-verify`, could not use the backend the dogfood pass
    /// needs — the one flow this feature was built for.
    #[test]
    fn every_spelling_of_a_throwaway_directory_is_admitted() {
        let resolved_temp = std::env::temp_dir()
            .canonicalize()
            .expect("the system temp dir exists");
        let forms: Vec<(&str, std::path::PathBuf)> = vec![
            ("std::env::temp_dir()", std::env::temp_dir().join("holon-x")),
            ("canonicalised temp dir", resolved_temp.join("holon-x")),
            ("/tmp", std::path::PathBuf::from("/tmp/holon-live-verify")),
            (
                "/private/tmp",
                std::path::PathBuf::from("/private/tmp/holon-live-verify"),
            ),
        ];
        for (what, dir) in forms {
            let selected = select_backend(INTEGRATION_SECRET_SERVICE, &dir, Some("memory"), None);
            assert!(
                selected.is_ok(),
                "{what} names a throwaway directory ({}) and must be admitted; got {:?}",
                dir.display(),
                selected.map(|_| ()).unwrap_err().to_string()
            );
        }
    }

    /// The directory need not exist yet — a first boot creates it. Resolving
    /// only the part that does exist is what keeps the rule usable before the
    /// session has written anything.
    #[test]
    fn a_throwaway_directory_that_does_not_exist_yet_is_admitted() {
        let dir = std::env::temp_dir().join("holon-not-created-yet-1a2b3c/config");
        assert!(
            !dir.exists(),
            "precondition: this rung is about a path with no directory behind it"
        );
        assert!(
            select_backend(INTEGRATION_SECRET_SERVICE, &dir, Some("memory"), None).is_ok(),
            "a config dir the boot is about to create must be judged by where it IS, not by \
             whether it exists yet"
        );
    }

    /// A near-miss neighbour of a throwaway root is not one. `starts_with` on a
    /// `Path` compares COMPONENTS, and this states that rather than trusting
    /// it.
    #[test]
    fn a_sibling_of_a_throwaway_root_is_not_throwaway() {
        assert!(
            select_backend(
                INTEGRATION_SECRET_SERVICE,
                std::path::Path::new("/tmpfoo/holon"),
                Some("memory"),
                None,
            )
            .map(|_| ())
            .is_err(),
            "'/tmpfoo' merely starts with the same letters as '/tmp'"
        );
    }

    /// The refusal that keeps this out of a real install. A silent fall back to
    /// the platform keychain would be worse than either outcome: the session
    /// asked not to touch the login keychain and would be writing to it.
    #[test]
    fn a_real_config_dir_refuses_the_in_memory_backend() {
        let err = select_backend(
            INTEGRATION_SECRET_SERVICE,
            std::path::Path::new("/Users/someone/.config/holon"),
            Some("memory"),
            None,
        )
        .map(|_| ())
        .expect_err("a config dir that is not throwaway must refuse, not degrade quietly");
        let msg = format!("{err}");
        assert!(msg.contains("temp"), "the message names the rule: {msg}");
        assert!(
            msg.contains(BACKEND_UNSAFE_ENV),
            "and the one way to mean it anyway: {msg}"
        );
    }

    #[test]
    fn an_explicit_acknowledgement_admits_any_config_dir() {
        let selected = select_backend(
            INTEGRATION_SECRET_SERVICE,
            std::path::Path::new("/Users/someone/.config/holon"),
            Some("memory"),
            Some(BACKEND_UNSAFE_ACK),
        )
        .expect("the acknowledgement is the documented escape");
        assert!(selected.disclosure.is_some(), "and it still discloses");
    }

    /// A near-miss acknowledgement is a refusal.
    /// `HOLON_..._ALLOW_ANY_CONFIG_DIR=1` is what someone types when they
    /// have not read the rule.
    #[test]
    fn a_near_miss_acknowledgement_is_not_one() {
        assert!(
            select_backend(
                INTEGRATION_SECRET_SERVICE,
                std::path::Path::new("/Users/someone/.config/holon"),
                Some("memory"),
                Some("1"),
            )
            .is_err()
        );
    }

    #[test]
    fn an_unknown_backend_name_is_refused_rather_than_guessed() {
        let err = select_backend(
            INTEGRATION_SECRET_SERVICE,
            std::path::Path::new("/tmp"),
            Some("keyring"),
            None,
        )
        .map(|_| ())
        .expect_err("an unrecognized backend must stop the boot");
        assert!(format!("{err}").contains("keyring"));
    }

    #[test]
    fn android_store_names_the_unmet_precondition() {
        let kc = UnavailableKeychainStore::android_ndk_context_unwired();
        let msg = kc.load("owner").unwrap_err().to_string();
        assert!(msg.contains("android"), "{msg}");
        assert!(msg.contains("ndk_context"), "{msg}");
    }
}
