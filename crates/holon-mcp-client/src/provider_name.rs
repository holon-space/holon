//! The parsed identity of one connection.
//!
//! A provider id used to be a `&'static str`, which encoded a real fact: the
//! name came from the compiled-in bundle, so it could not be anything a user
//! typed. Once a file on disk can introduce a connection that guarantee is
//! gone, and a type that still says `'static` would assert it falsely.
//!
//! [`ProviderName`] replaces the guarantee rather than dropping it: a value of
//! this type is a name that has been PARSED, so it cannot be empty, cannot
//! contain a path separator, and cannot be `.` or `..`. Every path the loader
//! composes from a provider id — the state file, the sidecar, the keychain
//! account — takes this type, so traversal through a crafted filename is
//! unrepresentable instead of being defended against at each site.

use std::borrow::Borrow;
use std::sync::Arc;

/// Longest name accepted. A provider id becomes a filename (`<name>.yaml`,
/// `<name>.state.toml`) and a keychain account, so it is bounded well below
/// any of their limits rather than at them.
const MAX_LEN: usize = 64;

/// A connection's id, proved to be usable as one path segment.
///
/// `Arc<str>` rather than `String`: a row carries the name into every settings
/// surface and every disclosure, so cloning is common and none of those clones
/// mutate it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderName(Arc<str>);

impl ProviderName {
    /// Parse `raw` into a provider id, or say exactly why it is not one.
    ///
    /// The accepted shape is `[a-z0-9][a-z0-9_-]*`, at most [`MAX_LEN`] bytes.
    /// Lowercase-only is what keeps a case-insensitive filesystem from making
    /// `GitHub.yaml` and `github.yaml` two names for one file.
    pub fn parse(raw: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(!raw.is_empty(), "a provider name must not be empty");
        anyhow::ensure!(
            raw.len() <= MAX_LEN,
            "provider name '{raw}' is {} bytes; the limit is {MAX_LEN}",
            raw.len()
        );
        let mut chars = raw.chars();
        let first = chars.next().expect("non-empty was just checked");
        anyhow::ensure!(
            first.is_ascii_lowercase() || first.is_ascii_digit(),
            "provider name '{raw}' must start with a lowercase letter or a digit, not '{first}'"
        );
        for c in chars {
            anyhow::ensure!(
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_',
                "provider name '{raw}' contains '{c}'; a name may hold only lowercase letters, \
                 digits, '-' and '_'"
            );
        }
        Ok(Self(Arc::from(raw)))
    }

    /// Parse a name this build compiled in.
    ///
    /// Separate from [`parse`](Self::parse) so a bundled name that stops being
    /// a legal id fails at the one boot-time site that knows it came from the
    /// bundle, naming the bundle — rather than surfacing as a mysterious
    /// user-config error.
    pub fn bundled(raw: &'static str) -> Self {
        Self::parse(raw).unwrap_or_else(|e| {
            panic!("BUNDLED_SIDECARS holds '{raw}', which is not a usable provider name: {e:#}")
        })
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProviderName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::ops::Deref for ProviderName {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

/// Lets a `HashMap<ProviderName, _>` be looked up by `&str`, so the store's
/// read paths keep taking a plain name.
impl Borrow<str> for ProviderName {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ProviderName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl PartialEq<str> for ProviderName {
    fn eq(&self, other: &str) -> bool {
        &*self.0 == other
    }
}

impl PartialEq<&str> for ProviderName {
    fn eq(&self, other: &&str) -> bool {
        &*self.0 == *other
    }
}

impl PartialEq<ProviderName> for &str {
    fn eq(&self, other: &ProviderName) -> bool {
        *self == &*other.0
    }
}
