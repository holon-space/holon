//! The one HTTP client every outbound connection call uses.
//!
//! Checking the URL a sidecar declares is only half a guard. `reqwest`'s
//! default policy follows up to ten redirects and does not look at the scheme,
//! so an `https://` endpoint answering `302 Location: http://…` gets the next
//! request — carrying the same auth header — sent in the clear, and the body
//! that becomes rows in the vault arrives from whoever answered. The load-time
//! guard cannot see that: it only ever sees the first URL.
//!
//! So the rule is applied twice, from one definition
//! ([`crate::integration_config::is_secure_url`]): once when the sidecar loads,
//! and once per hop at request time.

/// The phrase every redirect refusal carries.
///
/// Named as a constant because the distinction it marks is easy to lose: a
/// request to an unreachable host fails on its own, so "the call failed" is no
/// evidence the hop was blocked. A test asserts on THIS, not on failure.
pub(crate) const REDIRECT_REFUSED: &str = "refused a redirect";

/// How long one request may take, from connecting until the last body byte.
pub const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The largest response body read, counted as it streams in.
pub const MAX_RESPONSE_BODY_BYTES: usize = 64 * 1024 * 1024;

/// A client that refuses any redirect hop leaving https (loopback excepted)
/// and gives up on a request after [`REQUEST_TIMEOUT`].
///
/// The refusal names neither the target nor the origin: a redirect target is
/// attacker-chosen text and a connection's URL can itself be a credential, and
/// this message reaches logs.
pub(crate) fn secure_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if !crate::integration_config::is_secure_url(attempt.url()) {
                let scheme = attempt.url().scheme().to_string();
                return attempt.error(format!(
                    "{REDIRECT_REFUSED} to a {scheme}:// address — a connection must stay on \
                     https (or loopback), and following this hop would send its credentials in \
                     the clear. The target is not quoted here because it is chosen by the peer."
                ));
            }
            // Keep the default hop budget: a policy that allowed unlimited
            // same-scheme hops would trade one hazard for a redirect loop.
            if attempt.previous().len() >= 10 {
                return attempt.error(format!("{REDIRECT_REFUSED}: too many redirects"));
            }
            attempt.follow()
        }))
        .build()
        .expect("a reqwest client with a redirect policy and a timeout must build")
}

/// The body of `resp` as text, refused once more than
/// [`MAX_RESPONSE_BODY_BYTES`] have arrived. Bytes that are not UTF-8 become
/// U+FFFD, as reqwest's `text()` does without its `charset` feature.
pub(crate) async fn read_text(mut resp: reqwest::Response) -> anyhow::Result<String> {
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| anyhow::anyhow!(describe(e)))?
    {
        anyhow::ensure!(
            body.len() + chunk.len() <= MAX_RESPONSE_BODY_BYTES,
            "the response body is larger than MAX_RESPONSE_BODY_BYTES \
             ({MAX_RESPONSE_BODY_BYTES} bytes); reading stopped there"
        );
        body.extend_from_slice(&chunk);
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// A reqwest error plus its cause chain, with every URL stripped.
///
/// `reqwest`'s own `Display` gives only the outermost layer, so a redirect the
/// policy refused reads as the bare "error following redirect" and the REASON
/// — the only part that says what was wrong — is left in `source()`.
///
/// `without_url` is applied first so a URL that is itself a credential does
/// not ride along; the cause chain is provider text and callers redact it
/// again on the way out.
pub(crate) fn describe(e: reqwest::Error) -> String {
    let timed_out = e.is_timeout();
    let stripped = e.without_url();
    let mut out = stripped.to_string();
    let mut cause: Option<&dyn std::error::Error> = std::error::Error::source(&stripped);
    while let Some(c) = cause {
        out.push_str(": ");
        out.push_str(&c.to_string());
        cause = c.source();
    }
    if timed_out {
        format!("no complete response within REQUEST_TIMEOUT ({REQUEST_TIMEOUT:?}): {out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    /// The constant is what a test can assert on, so it must actually appear
    /// in what the policy emits. Guards against someone rewording the message
    /// and leaving the marker behind.
    #[test]
    fn the_marker_is_part_of_the_refusal_text() {
        let msg = format!(
            "{} to a http:// address — a connection must stay on https",
            super::REDIRECT_REFUSED
        );
        assert!(msg.contains(super::REDIRECT_REFUSED));
    }
}
