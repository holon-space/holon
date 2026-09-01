//! Credential file locations, confined to the profile they belong to.
//!
//! A sidecar declares WHERE its secrets live. Resolving those declarations
//! against `$HOME` makes the location a property of the machine rather than of
//! the running profile: an instance launched with `HOLON_CONFIG_DIR` pointing
//! at a throwaway directory still reaches the real user's OAuth refresh token
//! and syncs their real account.
//!
//! So a declared path is not a path. It is parsed here into a [`ConfinedPath`],
//! which exists only for a location inside the [`CredentialRoot`] it was
//! resolved against, and every credential read in this crate takes that type.
//! A declaration that names somewhere else is refused loudly at resolution
//! time — before any file is opened and before any transport is built.
//!
//! The accepted forms, all of which land under the root:
//!
//! - `${CONFIG_DIR}/gcal-refresh-token` — the explicit form the bundled
//!   sidecars use.
//! - `gcal-refresh-token` — a bare relative path, resolved against the root.
//! - an absolute path that lies inside the root (an installed sidecar being
//!   explicit about one profile).
//!
//! Everything else — a `~/` home reference, an absolute path elsewhere, a `..`
//! that climbs out, a link at a directory along the way or at the credential's
//! own name — is an escape and is refused. The link cases matter because every
//! read primitive follows links, so a path that only READS as confined is not
//! confined: the refusal is repeated in [`crate::rest_oauth2`] at the moment
//! the file is opened, which is the one that is load-bearing.

use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

/// The substitution a sidecar writes to name its own profile's config
/// directory. Spelled like the `${VAR}` references the rest of the sidecar
/// format uses, but resolved here rather than through
/// [`crate::integration_config::VarLookup`]: a lookup layer could be made to
/// yield any directory at all, which is the property this type exists to deny.
pub const CONFIG_DIR_VAR: &str = "${CONFIG_DIR}";

/// The active profile's config directory — the only place a sidecar's
/// credential files may live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialRoot(PathBuf);

/// A credential location proved to sit inside the [`CredentialRoot`] it was
/// resolved against. Constructed only by [`CredentialRoot::confine`], so a
/// value of this type IS the proof — there is no way to hand a credential
/// reader a path from somewhere else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfinedPath {
    root: PathBuf,
    path: PathBuf,
}

impl ConfinedPath {
    /// The absolute-under-the-root location to read.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The root this path was proved to sit inside. Carried so a disclosure can
    /// say which profile a missing credential belongs to, rather than leaving
    /// the reader to guess whether the sandbox or the real profile was meant.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl std::fmt::Display for ConfinedPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path.display())
    }
}

impl CredentialRoot {
    pub fn new(config_dir: impl Into<PathBuf>) -> Self {
        Self(normalize(&config_dir.into()))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Parse one declared credential path into a location inside this root.
    ///
    /// Fails loudly — never falls back to a machine-wide location — when the
    /// declaration names anywhere else. That refusal is the whole control: a
    /// profile that cannot reach outside its config dir cannot authenticate as
    /// somebody else's account.
    pub fn confine(&self, declared: &str) -> anyhow::Result<ConfinedPath> {
        let trimmed = declared.trim();
        anyhow::ensure!(
            !trimmed.is_empty(),
            "a credential file path is declared but empty; write \
             `{CONFIG_DIR_VAR}/<file>` to name a file in this profile's config directory"
        );

        let relative = if let Some(rest) = trimmed.strip_prefix(CONFIG_DIR_VAR) {
            rest.trim_start_matches('/')
        } else if trimmed.starts_with('~') {
            anyhow::bail!(
                "credential path '{declared}' is relative to the home directory, which is not \
                 this profile's config directory ('{}'). A home-relative credential makes every \
                 instance on the machine — including a sandbox launched with HOLON_CONFIG_DIR — \
                 authenticate as the same account. Write `{CONFIG_DIR_VAR}/<file>` instead.",
                self.0.display()
            )
        } else if Path::new(trimmed).is_absolute() {
            let absolute = normalize(Path::new(trimmed));
            anyhow::ensure!(
                absolute.starts_with(&self.0),
                "credential path '{declared}' is outside this profile's config directory ('{}'), \
                 so reading it would let this instance authenticate with another profile's \
                 credentials. Write `{CONFIG_DIR_VAR}/<file>` instead.",
                self.0.display()
            );
            self.assert_resolves_inside(&absolute, declared)?;
            return Ok(ConfinedPath {
                root: self.0.clone(),
                path: absolute,
            });
        } else {
            trimmed
        };

        let joined = normalize(&self.0.join(relative));
        anyhow::ensure!(
            joined.starts_with(&self.0),
            "credential path '{declared}' climbs out of this profile's config directory ('{}') \
             and resolves to '{}'. Write `{CONFIG_DIR_VAR}/<file>` instead.",
            self.0.display(),
            joined.display()
        );
        self.assert_resolves_inside(&joined, declared)?;
        Ok(ConfinedPath {
            root: self.0.clone(),
            path: joined,
        })
    }

    /// The lexical check above reads the path as WRITTEN. A link points
    /// somewhere the text does not say, so the same escape is available to
    /// anyone who can place one — at a directory ALONG the path, or at the
    /// credential's own name.
    ///
    /// Both are refused. The directory case is decided by containment (a config
    /// directory legitimately reached through a link — `/var` → `/private/var`
    /// on macOS — must still work, so both sides are canonicalized). The leaf
    /// case is refused outright rather than by containment: a credential is
    /// read from the profile that owns it, and a link at its name is how one
    /// profile borrows another's secret while every path check still passes.
    ///
    /// A path that does not exist yet resolves to nothing — the "not
    /// provisioned" case — and the lexical check stands alone until the file
    /// appears. [`crate::rest_oauth2`] repeats the leaf refusal at the moment
    /// it opens the file, which is where it is load-bearing; this one makes
    /// a link that is already in place fail the boot rather than the first
    /// request.
    fn assert_resolves_inside(&self, path: &Path, declared: &str) -> anyhow::Result<()> {
        if let Ok(meta) = std::fs::symlink_metadata(path) {
            anyhow::ensure!(
                !meta.file_type().is_symlink(),
                "credential path '{declared}' is a symbolic link. A credential is read from the \
                 profile that owns it ('{}'), never through a link that can point at another \
                 profile's secret. Replace the link with the credential itself.",
                self.0.display()
            );
        }

        let (Ok(root), Some(parent)) = (self.0.canonicalize(), path.parent()) else {
            return Ok(());
        };
        let Ok(parent) = parent.canonicalize() else {
            return Ok(());
        };
        anyhow::ensure!(
            parent.starts_with(&root),
            "credential path '{declared}' resolves through a link to '{}', outside this profile's \
             config directory ('{}'). Reading it would let this instance authenticate with \
             another profile's credentials.",
            parent.display(),
            root.display()
        );
        Ok(())
    }
}

/// Collapse `.` and `..` lexically. The credential file usually does not exist
/// yet (that is the "not provisioned" case), so `canonicalize` is unavailable
/// and containment is decided on the written path.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> CredentialRoot {
        CredentialRoot::new("/profiles/sandbox")
    }

    #[test]
    fn config_dir_substitution_lands_under_the_root() {
        let p = root().confine("${CONFIG_DIR}/gcal-refresh-token").unwrap();
        assert_eq!(p.path(), Path::new("/profiles/sandbox/gcal-refresh-token"));
        assert_eq!(p.root(), Path::new("/profiles/sandbox"));
    }

    #[test]
    fn a_bare_relative_path_lands_under_the_root() {
        let p = root().confine("gcal-refresh-token").unwrap();
        assert_eq!(p.path(), Path::new("/profiles/sandbox/gcal-refresh-token"));
    }

    #[test]
    fn a_home_relative_path_is_refused() {
        let err = root()
            .confine("~/.config/holon/gcal-refresh-token")
            .unwrap_err()
            .to_string();
        assert!(err.contains("home directory"), "{err}");
    }

    #[test]
    fn an_absolute_path_outside_the_root_is_refused() {
        let err = root()
            .confine("/Users/someone/.config/holon/gcal-refresh-token")
            .unwrap_err()
            .to_string();
        assert!(err.contains("outside"), "{err}");
    }

    #[test]
    fn an_absolute_path_inside_the_root_is_accepted() {
        let p = root().confine("/profiles/sandbox/nested/tok").unwrap();
        assert_eq!(p.path(), Path::new("/profiles/sandbox/nested/tok"));
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_the_credential_name_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let config_dir = base.path().join("profile");
        let victim = base.path().join("victim");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&victim).unwrap();
        std::fs::write(victim.join("token"), "victim-token").unwrap();
        std::os::unix::fs::symlink(victim.join("token"), config_dir.join("refresh-token")).unwrap();

        let err = CredentialRoot::new(&config_dir)
            .confine("${CONFIG_DIR}/refresh-token")
            .unwrap_err()
            .to_string();
        assert!(err.contains("symbolic link"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_out_of_the_root_is_refused() {
        let base = tempfile::tempdir().unwrap();
        let config_dir = base.path().join("profile");
        let elsewhere = base.path().join("elsewhere");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, config_dir.join("escape")).unwrap();

        let err = CredentialRoot::new(&config_dir)
            .confine("${CONFIG_DIR}/escape/refresh-token")
            .unwrap_err()
            .to_string();
        assert!(err.contains("through a link"), "{err}");
    }

    #[test]
    fn a_parent_traversal_is_refused() {
        let err = root()
            .confine("${CONFIG_DIR}/../other/gcal-refresh-token")
            .unwrap_err()
            .to_string();
        assert!(err.contains("climbs out"), "{err}");
    }
}
