//! The HTTP client setup shared by every service that talks to the network.

use std::time::Duration;

/// Install `ring` as the process-wide rustls crypto provider.
///
/// reqwest 0.13 is built with `rustls-no-provider`, so no provider is
/// registered by default and every TLS handshake would fail. Installing is a
/// process-global, once-only operation; a second call losing the race is
/// harmless because the winner installed the same provider.
fn install_crypto_provider() {
    static INSTALLED: std::sync::Once = std::sync::Once::new();
    INSTALLED.call_once(|| {
        if rustls::crypto::ring::default_provider()
            .install_default()
            .is_err()
        {
            log::debug!("http: rustls crypto provider was already installed");
        }
    });
}

/// A client identifying as obayebar, giving up on a request after `timeout`.
pub fn client(timeout: Duration) -> reqwest::Result<reqwest::Client> {
    install_crypto_provider();
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(concat!("obayebar/", env!("CARGO_PKG_VERSION")))
        .build()
}
