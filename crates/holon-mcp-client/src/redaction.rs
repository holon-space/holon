//! Keeping resolved secrets out of anything logged or displayed.
//!
//! # The redaction contract
//!
//! Redaction is by VALUE, not by shape. A capability URL carries its credential
//! in a path segment, which no query-string stripping can reach, and an
//! upstream error body can echo the request URL back — so a secret is stripped
//! wherever its bytes occur, in URLs and in arbitrary text alike.
//!
//! **How the match survives encoding.** A secret rarely reaches a message as
//! the bytes that were registered: the HTTP stack percent-encodes some
//! characters and not others, and a URL parser rewrites `\` to `/`. Both the
//! registered secret and the candidate text are therefore *canonicalized*
//! before matching — percent-decoded, `+` read as a space, `\` read as `/` —
//! and the span that matches is mapped back onto the ORIGINAL text, which is
//! what gets replaced. Matching on the decoded form is what makes partial and
//! mixed encodings (one character raw, another escaped) match a single
//! registered value, rather than requiring the exact encoding to have been
//! anticipated.
//!
//! **Registered** (and therefore stripped):
//!
//! - Every `${VAR}` value [`crate::integration_config`] expands, at
//!   config-resolution time. A sidecar references a variable *because* the
//!   value must stay out of the YAML, so provenance alone makes it a secret.
//! - The OAuth2 client secret and refresh token, when the provider resolves
//!   them from env, file, or keychain.
//! - Access tokens minted at runtime, registered as they are minted — a
//!   resource server that echoes the `Authorization` header into an error body
//!   would otherwise disclose one. These live in a ring of the last
//!   [`MINTED_RING`] tokens, so a long-lived process refreshing on a timer does
//!   not accumulate dead credentials forever.
//!
//! **Not covered**, deliberately:
//!
//! - Values shorter than [`MIN_SECRET_LEN`] bytes. A short value collides with
//!   ordinary message text, and rewriting every occurrence of `true` protects
//!   nothing while making errors unreadable.
//! - Secrets that reach a message under a transform other than URL encoding —
//!   base64, a hash, or a provider that echoes only a prefix. Matching those
//!   needs the transform, not the value.
//! - Double-encoded forms (`%253C` for `<`). Canonicalization decodes once, so
//!   a value escaped twice does not match. Decoding to a fixed point would
//!   collapse text that legitimately contains an escaped percent sign.
//! - Everything outside the `rest` transport and the OAuth2 provider. Other
//!   subsystems hold their own credentials and redact them their own way.
//!
//! The tradeoff is deliberate over-redaction: canonicalization maps distinct
//! byte sequences onto one form, and a registered value is replaced in every
//! string, so a `base_url` that is entirely `${VAR}` leaves errors naming
//! `<redacted>/things` rather than a host. Losing the host from a message beats
//! printing a credential into a log file.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::RwLock;

/// What a secret is replaced with.
const REDACTED: &str = "<redacted>";

/// Values shorter than this are never registered. See the module contract.
pub const MIN_SECRET_LEN: usize = 8;

/// How many runtime-minted access tokens stay redactable. A refresh supersedes
/// the previous token within a request or two, so a short ring covers the
/// in-flight window without growing without bound.
pub const MINTED_RING: usize = 3;

#[derive(Default)]
struct Registry {
    /// Canonicalized secrets resolved at configuration time. Never evicted:
    /// they stay valid for the life of the integration.
    permanent: Vec<Vec<u8>>,
    /// Canonicalized access tokens, most recent last, capped at
    /// [`MINTED_RING`].
    minted: VecDeque<Vec<u8>>,
}

/// The secrets one integration has resolved, ready to be stripped from any
/// string that leaves the process as a log line, an error, or a toast.
///
/// Cheap to clone and registrable from any thread: the `rest` transport holds
/// one, the OAuth2 provider behind it holds the same one, and a token minted
/// mid-request joins the set the moment it exists.
#[derive(Clone, Default)]
pub struct Redactor {
    registry: Arc<RwLock<Registry>>,
}

impl Redactor {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a secret for the life of the integration. Idempotent; values
    /// below [`MIN_SECRET_LEN`] are ignored.
    pub fn register(&self, value: &str) {
        let Some(canon) = registrable(value) else {
            return;
        };
        let mut reg = self.registry.write().expect("redactor lock poisoned");
        if !reg.permanent.contains(&canon) {
            reg.permanent.push(canon);
        }
    }

    /// Register an access token minted at runtime, evicting the oldest once
    /// [`MINTED_RING`] are held. Re-minting the same value moves it to the
    /// newest position, so a token still in use does not age out.
    pub fn register_minted(&self, value: &str) {
        let Some(canon) = registrable(value) else {
            return;
        };
        let mut reg = self.registry.write().expect("redactor lock poisoned");
        if let Some(at) = reg.minted.iter().position(|held| *held == canon) {
            reg.minted.remove(at);
        }
        if reg.minted.len() == MINTED_RING {
            reg.minted.pop_front();
        }
        reg.minted.push_back(canon);
    }

    /// Replace every registered secret occurrence in `text`, matching on the
    /// canonicalized form and replacing the corresponding span of the original.
    pub fn redact(&self, text: &str) -> String {
        let reg = self.registry.read().expect("redactor lock poisoned");
        if reg.permanent.is_empty() && reg.minted.is_empty() {
            return text.to_string();
        }
        let (canon, offsets) = canonicalize(text);

        let mut spans: Vec<(usize, usize)> = Vec::new();
        for secret in reg.permanent.iter().chain(reg.minted.iter()) {
            let mut from = 0;
            while let Some(at) = find(&canon[from..], secret) {
                let start = from + at;
                let end = start + secret.len();
                spans.push(char_aligned(text, offsets[start], offsets[end]));
                from = end;
            }
        }
        splice(text, spans)
    }

    /// [`Self::redact`] plus a generic layer: the whole query string goes,
    /// since a parameter can be sensitive without having come from a
    /// `${VAR}`.
    pub fn redact_url(&self, url: &str) -> String {
        let redacted = self.redact(url);
        match redacted.split_once('?') {
            Some((base, _)) => format!("{base}?{REDACTED}"),
            None => redacted,
        }
    }
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.registry.read() {
            Ok(reg) => write!(
                f,
                "Redactor({} permanent, {} minted)",
                reg.permanent.len(),
                reg.minted.len()
            ),
            Err(_) => write!(f, "Redactor(poisoned)"),
        }
    }
}

/// The canonical bytes of a value worth registering, or `None` when it is too
/// short to register.
///
/// Both forms must clear the floor: matching happens on the canonical bytes, so
/// a long value that canonicalizes short (`%41%42%43` → `ABC`) would rewrite
/// ordinary text everywhere the short form appears.
fn registrable(value: &str) -> Option<Vec<u8>> {
    let canon = canonicalize(value).0;
    (value.len() >= MIN_SECRET_LEN && canon.len() >= MIN_SECRET_LEN).then_some(canon)
}

/// Canonicalize `input` and record, for each canonical byte, where it started
/// in `input`. The returned offsets have one extra entry holding `input.len()`,
/// so a canonical span `[a, b)` maps to the original span
/// `[offsets[a], offsets[b])`.
///
/// Percent escapes decode; `+` reads as a space and `\` as `/`, the two
/// rewrites a URL parser applies on its own.
fn canonicalize(input: &str) -> (Vec<u8>, Vec<usize>) {
    let bytes = input.as_bytes();
    let mut canon = Vec::with_capacity(bytes.len());
    let mut offsets = Vec::with_capacity(bytes.len() + 1);
    let mut i = 0;
    while i < bytes.len() {
        let (byte, width) = match decode_escape(bytes, i) {
            Some(decoded) => (decoded, 3),
            None => (bytes[i], 1),
        };
        canon.push(match byte {
            b'+' => b' ',
            b'\\' => b'/',
            other => other,
        });
        offsets.push(i);
        i += width;
    }
    offsets.push(bytes.len());
    (canon, offsets)
}

/// The byte a `%XX` escape at `i` denotes, or `None` when `i` does not start a
/// well-formed escape.
fn decode_escape(bytes: &[u8], i: usize) -> Option<u8> {
    if bytes[i] != b'%' {
        return None;
    }
    let hi = hex_value(*bytes.get(i + 1)?)?;
    let lo = hex_value(*bytes.get(i + 2)?)?;
    Some(hi * 16 + lo)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Widen a span to the nearest character boundaries. A canonical match can land
/// mid-character only when the original held a multi-byte character the
/// canonical form does not; widening over-redacts by a character rather than
/// leaving part of the secret behind.
fn char_aligned(text: &str, mut start: usize, mut end: usize) -> (usize, usize) {
    while start > 0 && !text.is_char_boundary(start) {
        start -= 1;
    }
    while end < text.len() && !text.is_char_boundary(end) {
        end += 1;
    }
    (start, end)
}

/// Replace each span of `text` with the redaction marker, merging spans that
/// overlap so a nested match is not replaced twice.
fn splice(text: &str, mut spans: Vec<(usize, usize)>) -> String {
    if spans.is_empty() {
        return text.to_string();
    }
    spans.sort_unstable();

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end) in spans {
        if end <= cursor {
            continue;
        }
        let start = start.max(cursor);
        out.push_str(&text[cursor..start]);
        out.push_str(REDACTED);
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor(values: &[&str]) -> Redactor {
        let r = Redactor::new();
        for v in values {
            r.register(v);
        }
        r
    }

    #[test]
    fn redacts_a_secret_in_a_url_path_segment() {
        let r = redactor(&["tok-abcdefghijklmnop"]);
        assert_eq!(
            r.redact_url("https://api.example.com/c/tok-abcdefghijklmnop/feed"),
            "https://api.example.com/c/<redacted>/feed"
        );
    }

    #[test]
    fn redacts_a_secret_echoed_in_a_response_body() {
        let r = redactor(&["tok-abcdefghijklmnop"]);
        let body = r#"{"error":"no route for /c/tok-abcdefghijklmnop/feed"}"#;
        assert_eq!(
            r.redact(body),
            r#"{"error":"no route for /c/<redacted>/feed"}"#
        );
    }

    #[test]
    fn redacts_a_fully_percent_encoded_secret() {
        let r = redactor(&["tok|abcdefghijklmnop"]);
        assert_eq!(
            r.redact("no route for /c/tok%7Cabcdefghijklmnop/feed"),
            "no route for /c/<redacted>/feed"
        );
    }

    #[test]
    fn redacts_a_partially_encoded_secret() {
        // The wire form escapes `<` but leaves `|` alone — the mix no set of
        // pre-computed encodings anticipates.
        let r = redactor(&["tok|abc<defghijklmnop"]);
        assert_eq!(
            r.redact("no route for /c/tok|abc%3Cdefghijklmnop/feed"),
            "no route for /c/<redacted>/feed"
        );
    }

    #[test]
    fn redacts_a_secret_whose_backslash_the_url_parser_rewrote() {
        let r = redactor(&[r"tok\abcdefghijklmnop"]);
        assert_eq!(
            r.redact("no route for /c/tok/abcdefghijklmnop/feed"),
            "no route for /c/<redacted>/feed"
        );
    }

    #[test]
    fn redacts_the_form_encoded_form_of_a_secret() {
        let r = redactor(&["tok abcdefghijklmnop"]);
        assert_eq!(r.redact("q=tok+abcdefghijklmnop&x=1"), "q=<redacted>&x=1");
        assert_eq!(r.redact("/c/tok%20abcdefghijklmnop"), "/c/<redacted>");
    }

    #[test]
    fn strips_the_query_string_even_for_an_unregistered_parameter() {
        let r = redactor(&[]);
        assert_eq!(
            r.redact_url("https://api.example.com/token?client_secret=abc"),
            "https://api.example.com/token?<redacted>"
        );
    }

    #[test]
    fn a_short_value_is_not_registered() {
        let r = redactor(&["true"]);
        assert_eq!(r.redact("is it true?"), "is it true?");
    }

    #[test]
    fn a_secret_containing_another_is_replaced_whole() {
        let r = redactor(&["inner-secret", "prefix-inner-secret-suffix"]);
        assert_eq!(r.redact("prefix-inner-secret-suffix"), "<redacted>");
    }

    #[test]
    fn every_occurrence_is_replaced() {
        let r = redactor(&["tok-abcdefghijklmnop"]);
        assert_eq!(
            r.redact("tok-abcdefghijklmnop and tok-abcdefghijklmnop"),
            "<redacted> and <redacted>"
        );
    }

    #[test]
    fn text_around_a_secret_survives_multibyte_characters() {
        let r = redactor(&["tok-abcdefghijklmnop"]);
        assert_eq!(
            r.redact("naïve → tok-abcdefghijklmnop ← done"),
            "naïve → <redacted> ← done"
        );
    }

    #[test]
    fn a_value_that_canonicalizes_short_is_not_registered() {
        // Nine raw bytes, three canonical ones. Registering it would rewrite
        // every "ABC" in every message.
        let r = redactor(&["%41%42%43"]);
        assert_eq!(
            r.redact("ABC is an ordinary word"),
            "ABC is an ordinary word"
        );
        assert_eq!(r.redact("%41%42%43"), "%41%42%43");
    }

    #[test]
    fn a_minted_token_is_stripped_once_registered() {
        let r = redactor(&[]);
        let minted = "access-tok-7Hn4pQ2sVb9eLxTm";
        assert!(r.redact(minted).contains(minted));
        r.register_minted(minted);
        assert_eq!(r.redact(minted), "<redacted>");
    }

    #[test]
    fn the_minted_ring_evicts_the_oldest_token() {
        let r = redactor(&["permanent-config-secret"]);
        let tokens = [
            "access-tok-aaaaaaaaaaaa",
            "access-tok-bbbbbbbbbbbb",
            "access-tok-cccccccccccc",
            "access-tok-dddddddddddd",
        ];
        for t in tokens {
            r.register_minted(t);
        }
        // The fourth mint pushed the first out of the ring.
        assert_eq!(r.redact(tokens[0]), tokens[0]);
        for t in &tokens[1..] {
            assert_eq!(r.redact(t), "<redacted>", "still-live token {t} leaked");
        }
        // A configuration-time secret is never evicted by minting.
        assert_eq!(r.redact("permanent-config-secret"), "<redacted>");
    }

    #[test]
    fn re_minting_a_held_token_keeps_it_from_aging_out() {
        let r = redactor(&[]);
        let tokens = [
            "access-tok-aaaaaaaaaaaa",
            "access-tok-bbbbbbbbbbbb",
            "access-tok-cccccccccccc",
        ];
        for t in tokens {
            r.register_minted(t);
        }
        // The provider hands out the cached token again, then mints a new one.
        r.register_minted(tokens[0]);
        r.register_minted("access-tok-dddddddddddd");

        assert_eq!(
            r.redact(tokens[0]),
            "<redacted>",
            "still-live token evicted"
        );
        assert_eq!(r.redact(tokens[1]), tokens[1], "oldest token still held");
    }
}
