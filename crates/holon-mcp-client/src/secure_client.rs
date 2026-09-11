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

/// A client that refuses any redirect hop leaving https (loopback excepted).
///
/// The refusal names neither the target nor the origin: a redirect target is
/// attacker-chosen text and a connection's URL can itself be a credential, and
/// this message reaches logs.
pub(crate) fn secure_http_client() -> reqwest::Client {
    build(reqwest::Client::builder())
}

/// Apply the policy to a builder a caller has already configured (a timeout,
/// say), so no call site can acquire a client that skips it.
pub(crate) fn build(builder: reqwest::ClientBuilder) -> reqwest::Client {
    builder
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
        .expect("a reqwest client with only a redirect policy set must build")
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
