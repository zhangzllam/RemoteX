//! Shared Control connection policy for Desktop and both Agents.

use std::{net::SocketAddr, time::Duration};

pub const DEFAULT_CONTROL_URL: &str = "https://39.96.68.170:7443";
pub const LEGACY_CONTROL_URL: &str = "https://control.39-96-68-170.sslip.io";
pub const DEFAULT_RELAY_ADDRESS: &str = "39.96.68.170:7443";
pub const DEFAULT_RELAY_SERVER_NAME: &str = "relay.39-96-68-170.sslip.io";
const PERSONAL_CONTROL_ROOT: &[u8] = include_bytes!("../resources/personal-control-root.pem");

/// Only migrate our former built-in endpoint, never another self-hosted server.
#[must_use]
pub fn normalize_control_url(value: &str) -> String {
    let normalized = value.trim().trim_end_matches('/');
    if normalized == LEGACY_CONTROL_URL {
        DEFAULT_CONTROL_URL.to_owned()
    } else {
        normalized.to_owned()
    }
}

/// Build a client scoped to the chosen deployment. Its private root never enters
/// the OS trust store or the clients used for other self-hosted servers.
pub fn client(url: &str, timeout: Duration) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder().timeout(timeout);
    if url.trim().trim_end_matches('/') == DEFAULT_CONTROL_URL {
        builder = builder
            .no_proxy()
            .tls_built_in_root_certs(false)
            .add_root_certificate(reqwest::Certificate::from_pem(PERSONAL_CONTROL_ROOT)?)
            .redirect(reqwest::redirect::Policy::none());
    }
    builder.build()
}

/// The deployed Relay uses an IP endpoint; its DNS name is retained for TLS.
/// Map old credentials as well, avoiding Fake-IP DNS for this one known host.
#[must_use]
pub fn known_relay_address(value: &str) -> Option<SocketAddr> {
    if value == "relay.39-96-68-170.sslip.io:7443" || value == DEFAULT_RELAY_ADDRESS {
        Some(SocketAddr::from(([39, 96, 68, 170], 7443)))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migration_is_limited_to_the_exact_previous_default() {
        assert_eq!(
            normalize_control_url(&format!(" {LEGACY_CONTROL_URL}/ ")),
            DEFAULT_CONTROL_URL
        );
        for custom in [
            "https://control.39-96-68-170.sslip.io:8443",
            "https://control.39-96-68-170.sslip.io/private",
            "https://control.39-96-68-170.sslip.io.other.test",
            "https://other.test",
        ] {
            assert_eq!(normalize_control_url(custom), custom);
        }
    }

    #[test]
    fn custom_relay_addresses_are_not_overridden() {
        assert!(known_relay_address("relay.39-96-68-170.sslip.io:9443").is_none());
        assert!(known_relay_address("other.test:7443").is_none());
        assert_eq!(
            known_relay_address("relay.39-96-68-170.sslip.io:7443")
                .unwrap()
                .to_string(),
            DEFAULT_RELAY_ADDRESS
        );
    }

    #[tokio::test]
    #[ignore = "requires the deployed personal Control and open TCP 7443"]
    async fn deployed_control_verifies_and_reports_ready() {
        let response = client(DEFAULT_CONTROL_URL, Duration::from_secs(8))
            .unwrap()
            .get(format!("{DEFAULT_CONTROL_URL}/ready"))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert_eq!(
            response.json::<serde_json::Value>().await.unwrap()["status"],
            "ready"
        );
    }
}
