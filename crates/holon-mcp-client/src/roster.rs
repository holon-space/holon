//! The PRESENCE axis: which connections exist at all.
//!
//! It used to be a compile-time constant, so a file on disk could neither
//! introduce a connection nor switch one on. The second half of that is the
//! security property and is unchanged — enablement is still the state store's
//! decision and nothing else's. The first half is what ADR 0034 §6 asks for and
//! what this module supplies: a user's own `<name>.yaml` names a connection the
//! build does not ship.
//!
//! A roster is a VALUE computed once per scan, so the store, the loader and the
//! settings surface all read one answer rather than three re-derivations that
//! can disagree.
//!
//! Order is part of the answer, because the settings list renders in it: the
//! bundle first, in its declared order, then introduced connections in
//! file-name order. Adding a file never reshuffles the rows above it.

use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

use crate::bundled_sidecars::BUNDLED_SIDECARS;
use crate::bundled_sidecars::BundledSidecar;
use crate::provider_name::ProviderName;

/// Where one connection's content comes from.
#[derive(Debug, Clone)]
pub enum ConnectionSource {
    /// Compiled in. An installed file with this stem OVERRIDES its content
    /// (the existing schema-gated rule) but does not introduce anything.
    Bundled(&'static BundledSidecar),
    /// Introduced by a file. Nothing reviewed this content, which is why the
    /// loader applies rules to it that a bundled sidecar does not need.
    Installed { path: PathBuf },
}

/// One connection the build knows about, by either route.
#[derive(Debug, Clone)]
pub struct ConnectionEntry {
    pub name: ProviderName,
    pub source: ConnectionSource,
}

impl ConnectionEntry {
    /// Whether this connection's content came from a file rather than the
    /// bundle. The rules that apply only to unreviewed content read this
    /// rather than re-deriving it from the name.
    pub fn is_introduced(&self) -> bool {
        matches!(self.source, ConnectionSource::Installed { .. })
    }
}

/// A file in the integrations directory that named no connection, and why.
///
/// Carried rather than logged, for the reason the rest of this crate carries
/// its disclosures: a file the user put there deliberately, silently doing
/// nothing, is the failure this code exists to refuse.
#[derive(Debug, Clone)]
pub struct RejectedFile {
    pub path: PathBuf,
    pub reason: String,
}

/// The presence axis, in render order.
#[derive(Debug, Clone)]
pub struct ConnectionRoster {
    entries: Vec<ConnectionEntry>,
    rejected: Vec<RejectedFile>,
}

impl ConnectionRoster {
    /// Scan `dir` and union it with the bundle.
    ///
    /// A missing directory just means no installed files. A single unusable
    /// file is rejected and disclosed, never fatal: one bad name must not take
    /// down every other connection the user has.
    pub fn scan(dir: &Path) -> anyhow::Result<Self> {
        let (installed, mut rejected) =
            crate::integration_config::scan_installed_sidecars_reporting(dir)?;

        let mut entries: Vec<ConnectionEntry> = BUNDLED_SIDECARS
            .iter()
            .map(|s| ConnectionEntry {
                name: ProviderName::bundled(s.provider),
                source: ConnectionSource::Bundled(s),
            })
            .collect();
        // Held back rather than pushed straight into `entries`: whether one is
        // admissible depends on the OTHER introduced names, so the namespace
        // boundary below needs the whole set.
        let mut candidates: Vec<(ProviderName, PathBuf)> = Vec::new();
        // BTreeMap so introduced connections land in file-name order, which is
        // the order the settings list appends them in.
        let by_stem: BTreeMap<&String, &Vec<(PathBuf, String)>> = installed.iter().collect();
        for (stem, files) in by_stem {
            if crate::bundled_sidecars::bundled_sidecar(stem).is_some() {
                // A bundled stem OVERRIDES content and introduces nothing, so
                // it is already in the roster exactly once.
                continue;
            }
            let name = match ProviderName::parse(stem) {
                Ok(name) => name,
                Err(e) => {
                    for (path, _) in files {
                        rejected.push(RejectedFile {
                            path: path.clone(),
                            reason: format!(
                                "its file name is not a usable connection name: {e}. Rename the \
                                 file; the name becomes a file path and a keychain account, so it \
                                 may hold only lowercase letters, digits, '-' and '_'."
                            ),
                        });
                    }
                    continue;
                }
            };
            // Two files for one introduced name: nothing can pick between
            // them, and picking by scan order would be the silent choice this
            // crate refuses. Both are rejected, and the name is NOT introduced.
            if files.len() > 1 {
                for (path, _) in files {
                    rejected.push(RejectedFile {
                        path: path.clone(),
                        reason: format!(
                            "'{name}' has {} installed files and there is no rule that picks \
                             between them — delete all but one",
                            files.len()
                        ),
                    });
                }
                continue;
            }
            let (path, content) = &files[0];
            // A connection can collide with ITSELF: `${MY_TOKEN}` and
            // `${MY.TOKEN}` are two spellings `secret_account` folds onto one
            // entry, both inside this connection's own namespace, with no
            // second provider anywhere for the nesting rule below to see.
            //
            // Refused rather than served by a single field. The two spellings
            // do resolve to one credential, so one field would LOOK right —
            // but a field declares only ONE `env_override`, so an export of
            // the other spelling would be in force while the field still
            // rendered as editable. A user typing into a field whose value
            // nothing reads is the silent-degradation tier; renaming one
            // reference is the small, obvious fix, and this says which two.
            if let Some(why) = self_colliding_accounts(content) {
                rejected.push(RejectedFile {
                    path: path.clone(),
                    reason: why,
                });
                continue;
            }
            candidates.push((name, path.clone()));
        }

        // The secret namespace is a BOUNDARY, and two providers whose prefixes
        // nest do not have one: `todoist-api` owns `todoist_api_`, which lies
        // wholly inside the bundled `todoist_`, so every variable it may
        // legally reference is also a variable the bundled connection's own
        // check would accept — including the account holding the user's real
        // token. Refused at admission rather than judged per reference,
        // because no reference-level test can separate the two once the
        // prefixes nest.
        let claimed: Vec<(String, String)> = BUNDLED_SIDECARS
            .iter()
            .map(|s| {
                let name = ProviderName::bundled(s.provider);
                (secret_namespace_prefix(&name), name.to_string())
            })
            .chain(
                candidates
                    .iter()
                    .map(|(name, _)| (secret_namespace_prefix(name), name.to_string())),
            )
            .collect();

        for (name, path) in candidates {
            let mine = secret_namespace_prefix(&name);
            // The introduced connection always loses. A file the user dropped
            // must never be able to refuse a connection this build ships, and
            // two introduced names that nest leave nothing to pick between —
            // both hit this arm, which is the same answer the two-files-one-
            // name case gives.
            let collision = claimed.iter().find(|(prefix, owner)| {
                *owner != name.to_string()
                    && (prefix.starts_with(&mine) || mine.starts_with(prefix))
            });
            match collision {
                Some((_, owner)) => rejected.push(RejectedFile {
                    path,
                    reason: format!(
                        "its secret namespace '{}*' overlaps the connection '{owner}', so a \
                         variable name cannot say which of the two a credential belongs to — and \
                         one of them could then read the other's. Rename this file to a name that \
                         is neither an extension nor a shortening of '{owner}'.",
                        mine.to_uppercase()
                    ),
                }),
                None => entries.push(ConnectionEntry {
                    name,
                    source: ConnectionSource::Installed { path },
                }),
            }
        }

        rejected.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Self { entries, rejected })
    }

    /// The FULL load-time verdict: [`Self::scan`], minus every introduced
    /// connection whose file the loader would then refuse for its CONTENT.
    ///
    /// A scan settles the rules that read a file's name and its neighbours. The
    /// rules that read what is inside it — the sidecar format, the identity
    /// column, the secrets a reference may name — are settled one seam later,
    /// when the content is loaded, and a caller that treats the scan as the
    /// whole answer disagrees with the booted app about which connections
    /// exist. That is what the enable script did: it reported an INTEGER-id
    /// connection switched on and the app refused it
    /// (`docs/Testing/bugfunnel/entries/
    /// 2026-09-12-the-enable-script-switches-on-a-connection-whose-id-column-the-loader-refuses.md`).
    ///
    /// Separate from `scan` rather than folded into it: the load path needs the
    /// refused entry to stay in the roster so its Settings row and its refusal
    /// toast still name the file. This is for the callers that need the verdict
    /// WITHOUT booting.
    pub fn scan_loadable(dir: &Path) -> anyhow::Result<Self> {
        let mut roster = Self::scan(dir)?;
        let installed = crate::integration_config::scan_installed_sidecars(dir)?;
        let mut kept: Vec<ConnectionEntry> = Vec::new();
        for entry in std::mem::take(&mut roster.entries) {
            let ConnectionSource::Installed { ref path } = entry.source else {
                kept.push(entry);
                continue;
            };
            let files = installed
                .get(entry.name.as_str())
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            match crate::integration_config::content_verdict(&entry, files.first())? {
                Ok(()) => kept.push(entry),
                Err(reason) => roster.rejected.push(RejectedFile {
                    path: path.clone(),
                    reason,
                }),
            }
        }
        roster.entries = kept;
        roster.rejected.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(roster)
    }

    /// A roster of the bundle alone — what a caller with no directory to scan
    /// gets, and what every pre-existing bundled-only path still sees.
    pub fn bundled_only() -> Self {
        Self {
            entries: BUNDLED_SIDECARS
                .iter()
                .map(|s| ConnectionEntry {
                    name: ProviderName::bundled(s.provider),
                    source: ConnectionSource::Bundled(s),
                })
                .collect(),
            rejected: Vec::new(),
        }
    }

    pub fn entries(&self) -> &[ConnectionEntry] {
        &self.entries
    }

    /// Files that named no connection. Every one is disclosed by the loader.
    pub fn rejected(&self) -> &[RejectedFile] {
        &self.rejected
    }

    /// The names, in render order.
    pub fn names(&self) -> Vec<ProviderName> {
        self.entries.iter().map(|e| e.name.clone()).collect()
    }

    pub fn get(&self, name: &str) -> Option<&ConnectionEntry> {
        self.entries.iter().find(|e| e.name == *name)
    }
}

/// Every `${VAR}` name the text references, in order of appearance.
///
/// Reads the FILE TEXT rather than a list of known fields: a reference can sit
/// in a call URL, an auth value, a query parameter or a child-process
/// argument, and a rule that enumerated fields would silently miss the next
/// place one can hide.
pub fn referenced_vars(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        match after.find('}') {
            Some(end) => {
                let var = &after[..end];
                if !var.is_empty() {
                    out.push(var.to_string());
                }
                rest = &after[end + 1..];
            }
            // Unterminated: the expander refuses it later with its own
            // message, and there is no name here to judge.
            None => break,
        }
    }
    out
}

/// Two DIFFERENT `${VAR}` spellings in `text` that address one keychain
/// account, as a refusal message naming both. `None` when every account the
/// text reaches is spelled exactly one way.
///
/// Repeating the SAME spelling is ordinary authoring and is not a collision —
/// what is refused is one account reachable under two names.
fn self_colliding_accounts(text: &str) -> Option<String> {
    let mut by_account: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for var in referenced_vars(text) {
        let spellings = by_account
            .entry(holon_secrets::secret_account(&var))
            .or_default();
        if !spellings.contains(&var) {
            spellings.push(var);
        }
    }
    let (account, spellings) = by_account.iter().find(|(_, s)| s.len() > 1)?;
    Some(format!(
        "it references {} for the one credential '{account}' — '.' and '_' are the same \
         separator in a keychain account, so those are two names for one entry. A settings field \
         can declare only ONE of them as the variable that overrides it, so the other would take \
         effect silently while the field still looked editable. Use one spelling throughout.",
        spellings
            .iter()
            .map(|s| format!("${{{s}}}"))
            .collect::<Vec<_>>()
            .join(" and ")
    ))
}

/// The variable-name prefix an introduced connection owns.
///
/// `my-own-thing` owns `my_own_thing_`, because hyphen and underscore are one
/// separator everywhere else a name is compared in this system. The trailing
/// separator is what stops `evil` from owning `eviltwin_key`.
pub fn secret_namespace_prefix(name: &ProviderName) -> String {
    format!("{}_", fold_separators(name.as_str()))
}

/// Lowercase with `.` and `-` both folded to `_`, for COMPARING a connection
/// name against a variable name.
///
/// Deliberately not `holon_secrets::secret_account`, which folds `.` only and
/// is the keychain account IDENTITY: widening that would rename existing
/// entries. Here nothing is stored, only compared, so a connection named
/// `my-own-thing` can own `${MY_OWN_THING_TOKEN}` without touching where any
/// secret lives.
fn fold_separators(s: &str) -> String {
    s.to_ascii_lowercase().replace(['.', '-'], "_")
}

/// Refuse an introduced connection that reaches for a secret outside its own
/// namespace.
///
/// This is the whole defence against the one-line exfiltration: a dropped file
/// naming another provider's credential in a URL it controls. A bundled
/// sidecar is exempt — it is reviewed, and the convention would rename the
/// variables every existing install already has.
pub fn check_secret_namespace(name: &ProviderName, text: &str) -> Result<(), String> {
    let prefix = secret_namespace_prefix(name);
    for var in referenced_vars(text) {
        let normalized = fold_separators(&var);
        if !normalized.starts_with(&prefix) {
            return Err(format!(
                "it references ${{{var}}}, which is outside its own secret namespace. A \
                 connection introduced by a file may reference only variables named \
                 '{}*' (case-insensitively, with '-' and '_' alike) — otherwise a \
                 dropped file could send another connection's credentials to an endpoint \
                 it chose. Rename the variable, or put this connection's own secret under \
                 '{}…' with `holon-secret set`.",
                prefix.to_uppercase(),
                prefix.to_uppercase()
            ));
        }
    }
    Ok(())
}
