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
        let mut reg = self.registry.write().expect("redactor lock poisoned");
        for canon in registrable(value).into_iter().chain(
            url_secret_parts(value)
                .iter()
                .filter_map(|p| registrable(p)),
        ) {
            if !reg.permanent.contains(&canon) {
                reg.permanent.push(canon);
            }
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
    /// canonicalized form and replacing the corresponding span of the original,
    /// then blank any `!`-marked path segment.
    ///
    /// The second layer covers what the first structurally cannot: a credential
    /// that rotates per request was never the value anyone registered. It
    /// applies to every string that leaves the transport, not only to URLs — an
    /// upstream that echoes the path back inside its error body leaks exactly
    /// as badly as a printed URL.
    pub fn redact(&self, text: &str) -> String {
        redact_marked_segments(&self.redact_registered(text))
    }

    fn redact_registered(&self, text: &str) -> String {
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

    /// [`Self::redact`] plus one more generic layer: the whole query string
    /// goes, since a parameter can be sensitive without having come from a
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

/// Blank every `!`-marked path segment, wherever it appears — a bare URL, or a
/// URL quoted inside an error message or an echoed response body.
///
/// Registration cannot reach a credential that ROTATES per request: by the time
/// it is printed it was never the value anyone registered, so matching on the
/// value finds nothing. Such a token is stripped structurally instead, on the
/// marker rather than on the value, so redaction does not depend on having been
/// told the secret first.
///
/// Only the credential run is replaced, not the rest of the segment, so
/// punctuation around a quoted URL survives and the message stays readable.
fn redact_marked_segments(text: &str) -> String {
    if !text.contains('!') {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while let Some(offset) = text[i..].find('!') {
        let bang = i + offset;
        let run_end = bytes[bang + 1..]
            .iter()
            .position(|b| !(b.is_ascii_alphanumeric() || *b == b'_' || *b == b'-'))
            .map_or(text.len(), |n| bang + 1 + n);
        out.push_str(&text[i..bang]);
        // A `!` in ordinary prose is followed by a space or nothing. Requiring
        // a credential-length run is what keeps "Error!" readable while still
        // blanking a marked token wherever it is quoted — a bare path, a query
        // value, or an echoed body that never carried the leading slash.
        if run_end - (bang + 1) >= MIN_SECRET_LEN {
            out.push_str(REDACTED);
        } else {
            out.push_str(&text[bang..run_end]);
        }
        i = run_end;
    }
    out.push_str(&text[i..]);
    out
}

/// The individually-secret pieces of a URL-shaped value: everything past the
/// authority, split on the delimiters a path and query are built from.
///
/// A capability URL is secret as a whole, but an upstream error rarely echoes
/// it whole — it quotes the path it was asked for, and matching only the full
/// string then finds nothing while the token stands in plain sight. Registering
/// the pieces makes the token strippable wherever it surfaces, in a URL or in
/// an echoed body. Non-URL values are left alone: splitting an opaque API key
/// would register fragments of it as secrets in their own right.
fn url_secret_parts(value: &str) -> Vec<&str> {
    let Some((_, after_scheme)) = value.split_once("://") else {
        return Vec::new();
    };
    let (authority, rest) = after_scheme.split_once('/').unwrap_or((after_scheme, ""));
    // The leading host label too: a capability URL can carry its credential as
    // a subdomain (`https://<token>.host/`) rather than as a path segment.
    let host_label = authority.split(['.', ':']).next().unwrap_or_default();
    std::iter::once(host_label)
        .chain(rest.split(['/', '?', '&', '=', '#']))
        // Both with and without a leading marker: an upstream may echo a marked
        // segment's token bare, and the marked and unmarked forms are different
        // byte strings to the matcher.
        .flat_map(|part| [part, part.strip_prefix('!').unwrap_or(part)])
        .filter(|part| looks_like_a_credential(part))
        .collect()
}

/// Whether a URL piece is worth registering in its own right.
///
/// A piece that reads as an ordinary lowercase word (`subscriptions`,
/// `calendars`, `gmail`) is structure, and registering it would blank that word
/// out of every unrelated diagnostic — the URL as a whole is still registered,
/// so nothing is lost by leaving it. A credential essentially always breaks the
/// pattern with a digit, a capital, or a separator.
fn looks_like_a_credential(part: &str) -> bool {
    part.bytes()
        .any(|b| b.is_ascii_digit() || b.is_ascii_uppercase() || b == b'-' || b == b'_')
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
    fn a_marked_path_segment_goes_without_having_been_registered() {
        // A per-request token was never registered and never could be, so the
        // marker is all there is to go on.
        // Synthetic, of the shape the real one has. A captured credential does
        // not belong in a fixture even after it has rotated.
        let out =
            Redactor::new().redact_url("https://host.example/!sYnTh3t1c_t0k3n-Xy9Q/api/list/7");
        assert_eq!(out, format!("https://host.example/{REDACTED}/api/list/7"));
    }

    #[test]
    fn a_marked_segment_echoed_inside_a_body_goes_too() {
        // An upstream quoting the path back in its error body leaks exactly as
        // badly as a printed URL, and registration reaches neither.
        let out =
            Redactor::new().redact(r#"{"error":"no route for /!sYnTh3t1c_t0k3n-Xy9Q/api/list/7"}"#);
        assert!(!out.contains("sYnTh3t1c"), "{out}");
        // The punctuation around the URL survives, so the message still reads.
        assert!(out.ends_with(r#"/api/list/7"}"#), "{out}");
    }

    #[test]
    fn a_marked_token_quoted_without_a_leading_slash_still_goes() {
        let out = Redactor::new().redact("retrying with token=!sYnTh3t1c_t0k3n-Xy9Q now");
        assert!(!out.contains("sYnTh3t1c"), "{out}");
    }

    #[test]
    fn an_exclamation_in_prose_survives() {
        // The credential-length floor is what separates a token from emphasis.
        let text = "boom! the request failed";
        assert_eq!(Redactor::new().redact(text), text);
    }

    #[test]
    fn a_static_marked_token_echoed_without_its_marker_is_still_stripped() {
        // Registration keeps both forms, so an upstream that quotes the token
        // bare does not slip past the value matcher.
        let r = redactor(&["https://host.example/!st4t1c_t0k3n_value/api"]);
        let out = r.redact("upstream said st4t1c_t0k3n_value was rejected");
        assert!(!out.contains("st4t1c_t0k3n_value"), "{out}");
    }

    #[test]
    fn an_unmarked_path_segment_stays_readable() {
        let out = Redactor::new().redact_url("https://host.example/api/list/7");
        assert_eq!(out, "https://host.example/api/list/7");
    }

    #[test]
    fn a_path_segment_of_a_whole_url_secret_is_stripped_on_its_own() {
        // The capability-URL shape where the WHOLE url is the `${VAR}`: an
        // upstream that echoes only the path it was asked for never repeats the
        // registered string, so the token has to be strippable by itself.
        let r = redactor(&["https://host.example/c/cap-7f3a9d2e4b8c1056/list"]);
        let out = r.redact("upstream failed for /c/cap-7f3a9d2e4b8c1056/list/42");
        assert!(!out.contains("cap-7f3a9d2e4b8c1056"), "{out}");
        assert!(out.contains(REDACTED), "{out}");
    }

    #[test]
    fn a_query_value_of_a_whole_url_secret_is_stripped_on_its_own() {
        let r = redactor(&["https://host.example/api?key=tok-19f4c8b27ae5"]);
        let out = r.redact("upstream echoed key=tok-19f4c8b27ae5");
        assert!(!out.contains("tok-19f4c8b27ae5"), "{out}");
    }

    #[test]
    fn a_credential_in_a_subdomain_is_stripped_on_its_own() {
        let r = redactor(&["https://cap-7f3a9d2e4b8c1056.host.example/api"]);
        let out = r.redact("upstream rejected cap-7f3a9d2e4b8c1056");
        assert!(!out.contains("cap-7f3a9d2e4b8c1056"), "{out}");
    }

    #[test]
    fn a_benign_path_word_does_not_become_a_global_redaction_trigger() {
        // Registering ordinary structure would blank the word out of every
        // unrelated message while protecting nothing.
        let r = redactor(&["https://host.example/subscriptions/calendars"]);
        assert_eq!(
            r.redact("listing subscriptions and calendars failed"),
            "listing subscriptions and calendars failed"
        );
    }

    #[test]
    fn the_host_of_a_url_secret_stays_readable_in_its_own_right() {
        // Only what follows the authority is registered piecewise, so a
        // diagnostic that names the host without the credential still reads.
        let r = redactor(&["https://host.example/c/cap-7f3a9d2e4b8c1056"]);
        assert_eq!(
            r.redact("could not resolve host.example"),
            "could not resolve host.example"
        );
    }

    #[test]
    fn an_opaque_secret_is_not_split_into_pieces() {
        // No `://`, so nothing is split: registering fragments of an API key
        // would rewrite ordinary text that happens to share one.
        let r = redactor(&["abcdefgh=ijklmnop"]);
        assert_eq!(r.redact("value abcdefgh stands"), "value abcdefgh stands");
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
