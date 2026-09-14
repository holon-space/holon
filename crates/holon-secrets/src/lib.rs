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

    /// Whether operations on this store reach the machine's keychain — the
    /// login keychain, Credential Manager, or the Secret Service — which is
    /// what [`grant_login_keychain`] gates.
    ///
    /// A store with no backend compiled in for the platform answers `false`,
    /// so it keeps refusing in its own words rather than being reported as a
    /// missing grant that would not have helped.
    fn reaches_machine_keychain(&self) -> bool {
        true
    }
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
    fn reaches_machine_keychain(&self) -> bool {
        false
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

/// Whether this process may reach the machine's real login keychain.
///
/// Default-deny: a process reaches the user's keychain only after a production
/// entry point granted it. A test binary has no way to grant it, which is the
/// point — the property holds for every wiring path, including ones written
/// after this line.
static LOGIN_KEYCHAIN_GRANTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Grant this process the machine's login keychain, from a production `main`.
///
/// The machine's keychain is a capability, not an ambient fact, because the
/// alternative was ambient and cost a developer twelve stray items and an
/// unclosable authorisation dialog. Every `main` that ships to a user calls
/// this once, before any wiring; nothing else may.
///
/// One-way and process-wide: a granted process never wants it back, and
/// `cfg(test)` does not reach across crates, so the arming cannot be a
/// compile-time condition.
pub fn grant_login_keychain() {
    LOGIN_KEYCHAIN_GRANTED.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Whether [`grant_login_keychain`] has been called in this process.
pub fn login_keychain_granted() -> bool {
    LOGIN_KEYCHAIN_GRANTED.load(std::sync::atomic::Ordering::SeqCst)
}

static MACHINE_KEYCHAIN_ATTEMPTS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// How many operations this process has asked the machine's keychain to
/// perform, admitted or refused.
///
/// A process without the grant is refused every one of them, so a non-zero
/// count says the wiring resolved the platform store instead of an injected
/// one. That is a leak with no symptom until the flow that uses the store runs,
/// which is why the count is a value a harness asserts on rather than a
/// debugging aid.
pub fn machine_keychain_attempts() -> u64 {
    MACHINE_KEYCHAIN_ATTEMPTS.load(std::sync::atomic::Ordering::SeqCst)
}

/// The platform backend behind the grant.
///
/// Every operation is refused with an `Err` until a production entry point
/// calls [`grant_login_keychain`]. Refusing at the OPERATION rather than at
/// construction is deliberate: a lazy DI provider that builds a store it never
/// uses is not a keychain access, and turning it into one would make the rule
/// fire on wiring shape instead of on behaviour.
struct GrantedKeychain {
    service: String,
    inner: Box<dyn KeychainStore>,
}

impl GrantedKeychain {
    fn admit(&self, op: &str, account: &str) -> Result<()> {
        MACHINE_KEYCHAIN_ATTEMPTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if login_keychain_granted() {
            return Ok(());
        }
        anyhow::bail!(
            "refusing to {op} {:?}/{account:?} in the machine's keychain: this process never \
             called `holon_secrets::grant_login_keychain`, which only a production entry point \
             does. A test that reaches this resolved the platform store instead of an injected \
             one — inject an `InMemoryKeychainStore`, or `ShareCredentials::in_memory` for \
             share custody, at the seam that built it.",
            self.service
        )
    }
}

impl KeychainStore for GrantedKeychain {
    fn reaches_machine_keychain(&self) -> bool {
        self.inner.reaches_machine_keychain()
    }

    fn store(&self, account: &str, secret: &[u8]) -> Result<()> {
        self.admit("store", account)?;
        self.inner.store(account, secret)
    }

    fn load(&self, account: &str) -> Result<Option<Vec<u8>>> {
        self.admit("load", account)?;
        self.inner.load(account)
    }

    fn delete(&self, account: &str) -> Result<()> {
        self.admit("delete", account)?;
        self.inner.delete(account)
    }
}

/// The backend for the current platform, filing every secret under `service`.
///
/// The ONE seam through which the machine's keychain is reachable, so the grant
/// above covers every caller — the share capability store, the owner key, the
/// OAuth2 credential resolver, the integration secrets — rather than each of
/// them carrying its own guard. On a platform with no backend compiled in the
/// store is returned unwrapped: it reaches nothing, and its own message is the
/// accurate one.
pub fn platform_keychain(service: &str) -> Box<dyn KeychainStore> {
    #[cfg(target_os = "macos")]
    let inner: Box<dyn KeychainStore> = Box::new(mac::MacKeychainStore::new(service));
    #[cfg(not(target_os = "macos"))]
    let inner: Box<dyn KeychainStore> = non_mac::platform_store(service);

    if !inner.reaches_machine_keychain() {
        return inner;
    }
    Box::new(GrantedKeychain {
        service: service.to_string(),
        inner,
    })
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

/// A file of fixture secrets to pre-load into the in-memory backend, so a
/// session driving the BUILT app reaches the flows that begin with a secret
/// already stored.
///
/// The alternative was typing one into the native masked dialog, which on macOS
/// is an AppleScript `display dialog` and needs System Events — an automation
/// permission an agent session does not have. Two dogfood passes therefore left
/// the "Stored in the keychain" row unverified.
pub const BACKEND_SEED_ENV: &str = "HOLON_SECRETS_MEMORY_SEED";

/// The backend a boot must use, and what it must say about it.
pub struct SelectedBackend {
    pub store: Box<dyn KeychainStore>,
    /// What a degraded-mode banner shows. `None` for the platform keychain,
    /// which is the undegraded case and needs no banner.
    pub disclosure: Option<MemoryDisclosure>,
}

/// What the in-memory backend owes the user, split by how it must be READ.
///
/// Prose can be summarised by cutting it; a path cannot. The banner carries a
/// character cap on its sentence, and while the seed file sat inside that
/// sentence the cap landed in the middle of the path — leaving only
/// `/private/var/folders/hc/…`, the half every temp path on the machine shares,
/// and never the file name, which is the half that says WHICH fixture planted
/// the credentials (`docs/Testing/bugfunnel/entries/
/// 2026-09-12-the-seeded-secrets-banner-cuts-its-own-file-path-and-miscounts-in-words.md`).
#[derive(Debug)]
pub struct MemoryDisclosure {
    /// The sentence. May be shortened.
    pub headline: String,
    /// The seed file, verbatim and uncapped. `None` when nothing was seeded.
    pub seed_path: Option<String>,
}

impl MemoryDisclosure {
    /// Whether the disclosure says this anywhere — sentence or payload.
    pub fn contains(&self, needle: &str) -> bool {
        self.headline.contains(needle)
            || self.seed_path.as_ref().is_some_and(|p| p.contains(needle))
    }
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
    let seed = std::env::var_os(BACKEND_SEED_ENV).map(std::path::PathBuf::from);
    select_backend(
        service,
        config_dir,
        std::env::var(BACKEND_ENV).ok().as_deref(),
        std::env::var(BACKEND_UNSAFE_ENV).ok().as_deref(),
        seed.as_deref(),
    )
}

/// The (account, secret) pairs a seed file names, in file order.
///
/// Each key is filed under [`secret_account`], because the file is written the
/// way a person writes a reference — `TODOIST_API_KEY` or `todoist.api_key` —
/// and both of those name ONE entry to every reader in the app. A seed that
/// landed under a third spelling would read as stored and resolve to nothing.
///
/// Every defect in the file is an `Err` naming the key, never a skipped entry:
/// a fixture that planted three of its four secrets and said nothing is the
/// failure this whole seam exists to avoid. The error text never carries a
/// value.
///
/// Public and separate from the store write so a fixture that owns its own
/// store — a windowed rung, which is handed one before the window opens —
/// plants the SAME accounts a boot would, rather than restating the folding
/// rule.
pub fn parse_seed_file(path: &std::path::Path) -> Result<Vec<(String, Vec<u8>)>> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        anyhow::anyhow!(
            "{BACKEND_SEED_ENV} names '{}', which cannot be read: {e}",
            path.display()
        )
    })?;
    let table: toml::Table = text.parse().map_err(|e| {
        anyhow::anyhow!(
            "{BACKEND_SEED_ENV} file '{}' is not a TOML table of `account = \"secret\"` lines: {e}",
            path.display()
        )
    })?;
    let mut claimed: std::collections::HashMap<String, &String> = std::collections::HashMap::new();
    let mut pairs = Vec::with_capacity(table.len());
    for (key, value) in &table {
        let secret = value.as_str().ok_or_else(|| {
            anyhow::anyhow!(
                "{BACKEND_SEED_ENV} file '{}' gives '{key}' a {} — a secret is a string, and \
                 guessing a spelling for one would plant a value nothing can read",
                path.display(),
                value.type_str()
            )
        })?;
        let account = secret_account(key);
        // Two spellings of ONE account. The folding is the point of this file —
        // `TODOIST_API_KEY` and `todoist.api_key` are one entry to every reader
        // in the app — so a file naming both plants one value and discards the
        // other, chosen by map order, while the banner counts two. Refused for
        // the same reason a self-colliding connection is: there is no rule that
        // picks, and a session reading a field as configured against a
        // credential it did not choose is the silent tier.
        if let Some(first) = claimed.insert(account.clone(), key) {
            anyhow::bail!(
                "{BACKEND_SEED_ENV} file '{}' names one account twice: '{first}' and '{key}' both \
                 fold onto '{account}', so one of the two values would be planted and the other \
                 silently dropped. Delete whichever spelling you did not mean.",
                path.display()
            );
        }
        pairs.push((account, secret.as_bytes().to_vec()));
    }
    Ok(pairs)
}

/// Pre-load `store` from a seed file, returning how many entries were planted.
fn seed_store(store: &dyn KeychainStore, path: &std::path::Path) -> Result<usize> {
    let pairs = parse_seed_file(path)?;
    for (account, secret) in &pairs {
        store.store(account, secret)?;
    }
    Ok(pairs.len())
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
    seed: Option<&std::path::Path>,
) -> Result<SelectedBackend> {
    match requested.unwrap_or_default().trim() {
        "" | "platform" => {
            // The one path by which a fixture value could reach a real
            // keychain, closed here rather than by convention: a seed is
            // meaningful only for a store that is thrown away.
            anyhow::ensure!(
                seed.is_none(),
                "{BACKEND_SEED_ENV} is set, but this session uses the system keychain — planting \
                 fixture secrets there would leave them in the login keychain after it exits. Set \
                 {BACKEND_ENV}=memory to seed a throwaway store, or unset {BACKEND_SEED_ENV}."
            );
            Ok(SelectedBackend {
                store: platform_keychain(service),
                disclosure: None,
            })
        }
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
            let store: Box<dyn KeychainStore> = Box::new(InMemoryKeychainStore::new());
            let mut disclosure = format!(
                "Secrets are held in memory for this session ({BACKEND_ENV}=memory). Nothing you \
                 type into a credential field is saved, and every stored secret is gone when \
                 Holon exits. Unset {BACKEND_ENV} to use the system keychain."
            );
            // Said on the same banner, because a credential that is already
            // there when the session opens is otherwise indistinguishable from
            // one the user stored — and these are somebody's fixtures. The
            // PATH leaves the sentence and travels on its own, so a cap on the
            // prose cannot eat the file name.
            let mut seed_path = None;
            if let Some(path) = seed {
                let planted = seed_store(store.as_ref(), path)?;
                let plural = if planted == 1 { "" } else { "s" };
                let verb = if planted == 1 { "was" } else { "were" };
                disclosure.push_str(&format!(
                    " {planted} seeded fixture secret{plural} {verb} pre-loaded \
                     ({BACKEND_SEED_ENV}) from this file; they are fixtures, not your \
                     credentials:"
                ));
                seed_path = Some(path.display().to_string());
            }
            Ok(SelectedBackend {
                store,
                disclosure: Some(MemoryDisclosure {
                    headline: disclosure,
                    seed_path,
                }),
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
    fn reaches_machine_keychain(&self) -> bool {
        false
    }
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

    /// The pin for "no test reaches the developer's login keychain". Stated
    /// over the ONE seam that can reach it, so it holds for every caller —
    /// including wiring paths written after this test.
    ///
    /// This test binary cannot call `grant_login_keychain`, so the account
    /// below is never looked up: the refusal happens before the backend call.
    #[test]
    fn every_login_keychain_operation_is_refused_without_a_grant() {
        assert!(
            !login_keychain_granted(),
            "a test binary must never hold the login-keychain grant; something called \
             grant_login_keychain, which only a production entry point may do"
        );

        let store = platform_keychain("space.holon.test-must-never-reach-this");
        for err in [
            store.load("founding-device").unwrap_err(),
            store.store("founding-device", b"x").unwrap_err(),
            store.delete("founding-device").unwrap_err(),
        ] {
            let msg = err.to_string();
            assert!(
                msg.contains("grant_login_keychain"),
                "the refusal must name the grant so the reader knows why: {msg}"
            );
            assert!(
                msg.contains("space.holon.test-must-never-reach-this"),
                "the refusal must name the service it refused: {msg}"
            );
        }
    }

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
        let selected = select_backend(INTEGRATION_SECRET_SERVICE, &dir, Some("memory"), None, None)
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
            let selected =
                select_backend(INTEGRATION_SECRET_SERVICE, &dir, Some("memory"), None, None);
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
            select_backend(INTEGRATION_SECRET_SERVICE, &dir, Some("memory"), None, None).is_ok(),
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
            None,
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
                None,
            )
            .is_err()
        );
    }

    /// A seed file names accounts the way a person writes a reference; both
    /// spellings must land on the entry the app reads back. Filing under a
    /// third spelling would make the row say "Stored in the keychain" about a
    /// value the `${VAR}` resolver cannot find.
    #[test]
    fn a_seed_file_lands_under_the_accounts_the_app_reads() {
        let dir = std::env::temp_dir().join("holon-secrets-seed-case");
        std::fs::create_dir_all(&dir).unwrap();
        let seed = dir.join("two.toml");
        std::fs::write(
            &seed,
            "TODOIST_API_KEY = \"fixture-a\"\n\"shopping.list_url\" = \"fixture-b\"\n",
        )
        .unwrap();

        let selected = select_backend(
            INTEGRATION_SECRET_SERVICE,
            &dir,
            Some("memory"),
            None,
            Some(&seed),
        )
        .expect("a throwaway session may be handed its fixture secrets");

        assert_eq!(
            selected
                .store
                .load(&secret_account("todoist.api_key"))
                .unwrap()
                .unwrap(),
            b"fixture-a",
            "`TODOIST_API_KEY` and `todoist.api_key` are one account everywhere else in the app"
        );
        assert_eq!(
            selected
                .store
                .load(&secret_account("SHOPPING_LIST_URL"))
                .unwrap()
                .unwrap(),
            b"fixture-b"
        );
        let disclosure = selected.disclosure.expect("still disclosed");
        assert!(
            disclosure.contains("2 seeded fixture secrets"),
            "the banner says how many were planted; got {disclosure:?}"
        );
        assert!(
            !disclosure.contains("fixture-a"),
            "and never quotes one of them"
        );
    }

    /// Two spellings of ONE account is a REFUSAL, for the same reason a
    /// connection that self-collides is: the folding makes them one entry, so
    /// the second write silently overwrites the first and the banner counts two
    /// seeded secrets for one stored value. Whichever value survived did so by
    /// map order, which is the silent choice this crate refuses to make.
    #[test]
    fn two_seed_keys_that_fold_to_one_account_are_refused() {
        let dir = std::env::temp_dir().join("holon-secrets-seed-collision-case");
        std::fs::create_dir_all(&dir).unwrap();
        let seed = dir.join("collide.toml");
        std::fs::write(
            &seed,
            "TODOIST_API_KEY = \"fixture-a\"\n\"todoist.api_key\" = \"fixture-b\"\n",
        )
        .unwrap();

        let err = parse_seed_file(&seed)
            .map(|_| ())
            .expect_err("one account named twice must stop the session, not be resolved by order");
        let msg = format!("{err:#}");
        for expected in ["TODOIST_API_KEY", "todoist.api_key", "todoist_api_key"] {
            assert!(
                msg.contains(expected),
                "the refusal must name BOTH spellings and the account they fold onto, because the \
                 fix is to delete one of them; `{expected}` is missing from: {msg}"
            );
        }
        assert!(
            !msg.contains("fixture-a") && !msg.contains("fixture-b"),
            "and must not quote either value: {msg}"
        );
    }

    /// A value that is not a string is a REFUSAL, not a skipped line: a fixture
    /// that planted three of its four secrets and said nothing sends the
    /// session hunting a bug in the app.
    #[test]
    fn a_seed_entry_that_is_not_a_string_is_refused_by_name() {
        let dir = std::env::temp_dir().join("holon-secrets-seed-bad-case");
        std::fs::create_dir_all(&dir).unwrap();
        let seed = dir.join("bad.toml");
        std::fs::write(&seed, "TODOIST_API_KEY = 12345\n").unwrap();

        let err = select_backend(
            INTEGRATION_SECRET_SERVICE,
            &dir,
            Some("memory"),
            None,
            Some(&seed),
        )
        .map(|_| ())
        .expect_err("a non-string secret must stop the session, not be dropped");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("TODOIST_API_KEY"),
            "and must name the key so the file can be fixed; got {msg}"
        );
    }

    /// The rule that keeps a fixture value out of the login keychain. Stated at
    /// the level of the ANSWER rather than of any caller, because there is no
    /// admissible way for a platform store to be handed a seed.
    #[test]
    fn a_seed_is_refused_outside_the_in_memory_backend() {
        let dir = std::env::temp_dir().join("holon-secrets-seed-platform-case");
        std::fs::create_dir_all(&dir).unwrap();
        let seed = dir.join("one.toml");
        std::fs::write(&seed, "TODOIST_API_KEY = \"fixture-a\"\n").unwrap();

        for requested in [None, Some("platform")] {
            let err = select_backend(
                INTEGRATION_SECRET_SERVICE,
                &dir,
                requested,
                None,
                Some(&seed),
            )
            .map(|_| ())
            .expect_err(
                "seeding the system keychain would leave fixture values in it after the session \
                 exits, so it must refuse rather than plant them",
            );
            assert!(
                format!("{err}").contains(BACKEND_ENV),
                "and must name the variable that makes the seed admissible"
            );
        }
    }

    #[test]
    fn an_unknown_backend_name_is_refused_rather_than_guessed() {
        let err = select_backend(
            INTEGRATION_SECRET_SERVICE,
            std::path::Path::new("/tmp"),
            Some("keyring"),
            None,
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
