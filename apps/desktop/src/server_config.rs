use serde::{Deserialize, Serialize};
use std::{fs::OpenOptions, io::Write, net::SocketAddr, path::PathBuf};
use tauri::{AppHandle, Manager};

const MAX_SERVER_CONFIG_SIZE: u64 = 32 * 1024;
const PLACEHOLDER_CONTROL_URL: &str = "https://control.example.com";
const BUILTIN_RELAY_CA_CERTIFICATE: &[u8] =
    include_bytes!("../resources/certificates/isrg-root-x1.pem");
pub use remotex_control_client::{
    DEFAULT_CONTROL_URL as DEFAULT_CONTROL_SERVER_URL, DEFAULT_RELAY_ADDRESS,
    DEFAULT_RELAY_SERVER_NAME,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerConfig {
    pub control_server_url: String,
    pub relay_address: String,
    pub relay_server_name: String,
    pub stun_address: String,
    pub ca_certificate_path: String,
    pub configured: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            control_server_url: DEFAULT_CONTROL_SERVER_URL.to_owned(),
            relay_address: DEFAULT_RELAY_ADDRESS.to_owned(),
            relay_server_name: DEFAULT_RELAY_SERVER_NAME.to_owned(),
            stun_address: String::new(),
            ca_certificate_path: String::new(),
            configured: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyControllerConfig {
    #[serde(default)]
    pub control_server_url: String,
    #[serde(default)]
    pub relay_address: String,
    #[serde(default)]
    pub relay_server_name: String,
    #[serde(default)]
    pub ca_certificate_path: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigCandidate {
    source: &'static str,
    config: ServerConfig,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerConfigState {
    pub config: ServerConfig,
    pub needs_setup: bool,
    pub migration_conflict: bool,
    pub candidates: Vec<ServerConfigCandidate>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCheckResult {
    pub ok: bool,
    pub endpoint: String,
    pub message: String,
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn load_server_config(
    app: AppHandle,
    legacy_controller: Option<LegacyControllerConfig>,
) -> Result<ServerConfigState, String> {
    if let Some(config) = read_server_config(&app)? {
        let completed = complete_managed_defaults(&app, config.clone())?;
        if completed != config {
            write_server_config(&app, &completed)?;
            crate::agent_management::sync_server_fields(&app, &completed)?;
        }
        return Ok(state_for(completed, false, Vec::new()));
    }

    let legacy_agent = crate::agent_management::legacy_server_config(&app)?;
    let agent_candidate = candidate_from_parts(
        "This device",
        &legacy_agent.server_url,
        "",
        "",
        &legacy_agent.ca_certificate_path,
        "",
    );
    let controller_candidate = legacy_controller.and_then(|legacy| {
        candidate_from_parts(
            "Outgoing connections",
            &legacy.control_server_url,
            &legacy.relay_address,
            &legacy.relay_server_name,
            &legacy.ca_certificate_path,
            "",
        )
    });
    let mut candidates = [agent_candidate, controller_candidate]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let matching_urls = candidates.len() == 2
        && normalize_url(&candidates[0].config.control_server_url)
            == normalize_url(&candidates[1].config.control_server_url);
    let conflicting_certificates = matching_urls
        && !candidates[0].config.ca_certificate_path.is_empty()
        && !candidates[1].config.ca_certificate_path.is_empty()
        && !candidates[0]
            .config
            .ca_certificate_path
            .eq_ignore_ascii_case(&candidates[1].config.ca_certificate_path);
    if matching_urls && !conflicting_certificates {
        let second = candidates.pop().expect("two migration candidates");
        let first = &mut candidates[0].config;
        if first.ca_certificate_path.is_empty() {
            first.ca_certificate_path = second.config.ca_certificate_path;
        }
        if first.relay_address.is_empty() {
            first.relay_address = second.config.relay_address;
        }
        if first.relay_server_name.is_empty() {
            first.relay_server_name = second.config.relay_server_name;
        }
        first.configured = !first.ca_certificate_path.is_empty();
    }

    match candidates.as_slice() {
        [] => {
            let config = managed_default_config(&app)?;
            write_server_config(&app, &config)?;
            crate::agent_management::sync_server_fields(&app, &config)?;
            Ok(state_for(config, false, Vec::new()))
        }
        [candidate] => {
            let config = complete_managed_defaults(&app, candidate.config.clone())?;
            if config.configured {
                write_server_config(&app, &config)?;
                crate::agent_management::sync_server_fields(&app, &config)?;
            }
            Ok(state_for(config, false, Vec::new()))
        }
        _ => Ok(state_for(ServerConfig::default(), true, candidates)),
    }
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn save_server_config(
    app: AppHandle,
    mut config: ServerConfig,
) -> Result<ServerConfig, String> {
    normalize_and_validate(&mut config)?;
    config.configured = true;
    write_server_config(&app, &config)?;
    crate::agent_management::sync_server_fields(&app, &config)?;
    Ok(config)
}

#[tauri::command]
pub async fn check_server_connection(config: ServerConfig) -> Result<ServerCheckResult, String> {
    check_server_connection_inner(config, true).await
}

#[tauri::command]
pub async fn check_control_server_connection(
    config: ServerConfig,
) -> Result<ServerCheckResult, String> {
    check_server_connection_inner(config, false).await
}

async fn check_server_connection_inner(
    config: ServerConfig,
    verify_relay: bool,
) -> Result<ServerCheckResult, String> {
    let mut checked = config;
    normalize_and_validate(&mut checked)?;
    let endpoint = format!("{}/ready", checked.control_server_url.trim_end_matches('/'));
    let response = control_client(
        &checked.control_server_url,
        std::time::Duration::from_secs(8),
    )
    .map_err(|error| format!("prepare secure server check: {error}"))?
    .get(&endpoint)
    .send()
    .await
    .map_err(|error| format!("could not reach the server: {error}"))?;
    if !response.status().is_success() {
        return Ok(ServerCheckResult {
            ok: false,
            endpoint,
            message: format!(
                "The server replied with HTTP {}.",
                response.status().as_u16()
            ),
        });
    }
    if verify_relay && !checked.relay_address.is_empty() && !checked.relay_server_name.is_empty() {
        check_relay_identity(&checked).await?;
    }
    Ok(ServerCheckResult {
        ok: true,
        endpoint,
        message: "Secure server is ready.".to_owned(),
    })
}

pub(crate) fn control_client(
    control_server_url: &str,
    timeout: std::time::Duration,
) -> Result<reqwest::Client, reqwest::Error> {
    remotex_control_client::client(control_server_url, timeout)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)]
pub fn reset_first_run(app: AppHandle) -> Result<ServerConfigState, String> {
    let config = read_server_config(&app)?.unwrap_or_default();
    Ok(ServerConfigState {
        needs_setup: true,
        config,
        migration_conflict: false,
        candidates: Vec::new(),
    })
}

pub fn read_server_config(app: &AppHandle) -> Result<Option<ServerConfig>, String> {
    let path = server_config_path(app)?;
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.len() > MAX_SERVER_CONFIG_SIZE => {
            return Err("Server configuration exceeds 32 KiB".to_owned());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("inspect server configuration: {error}")),
    }
    let data =
        std::fs::read(&path).map_err(|error| format!("read server configuration: {error}"))?;
    let config: ServerConfig = serde_json::from_slice(&data)
        .map_err(|error| format!("decode server configuration: {error}"))?;
    // Keep the stored URL intact here: load_server_config must see the change
    // so it persists migration and synchronizes the Agent settings.
    if config.configured {
        normalize_and_validate(&mut config.clone())?;
    }
    Ok(Some(config))
}

fn state_for(
    config: ServerConfig,
    migration_conflict: bool,
    candidates: Vec<ServerConfigCandidate>,
) -> ServerConfigState {
    ServerConfigState {
        needs_setup: !config.configured || migration_conflict,
        config,
        migration_conflict,
        candidates,
    }
}

fn candidate_from_parts(
    source: &'static str,
    control_server_url: &str,
    relay_address: &str,
    relay_server_name: &str,
    ca_certificate_path: &str,
    stun_address: &str,
) -> Option<ServerConfigCandidate> {
    let url = control_server_url.trim();
    let normalized_url = normalize_url(url);
    let is_unconfigured_managed_default = normalized_url == DEFAULT_CONTROL_SERVER_URL
        && relay_address.trim().is_empty()
        && relay_server_name.trim().is_empty()
        && ca_certificate_path.trim().is_empty();
    if url.is_empty()
        || normalized_url == PLACEHOLDER_CONTROL_URL
        || is_unconfigured_managed_default
    {
        return None;
    }
    let mut config = ServerConfig {
        control_server_url: url.to_owned(),
        relay_address: relay_address.trim().to_owned(),
        relay_server_name: relay_server_name.trim().to_owned(),
        ca_certificate_path: ca_certificate_path.trim().to_owned(),
        stun_address: stun_address.trim().to_owned(),
        configured: !ca_certificate_path.trim().is_empty(),
    };
    if normalize_and_validate_base(&mut config).is_err() {
        config.configured = false;
    }
    Some(ServerConfigCandidate { source, config })
}

fn normalize_and_validate(config: &mut ServerConfig) -> Result<(), String> {
    normalize_and_validate_base(config)?;
    if config.ca_certificate_path.trim().is_empty() {
        return Err("Choose the Relay CA certificate used by this server.".to_owned());
    }
    if config.ca_certificate_path.len() > 4_096 {
        return Err("The Relay CA certificate path is too long.".to_owned());
    }
    Ok(())
}

fn normalize_and_validate_base(config: &mut ServerConfig) -> Result<(), String> {
    config.control_server_url = normalize_url(&config.control_server_url);
    if config.control_server_url.len() > 2_048 {
        return Err("The server address is too long.".to_owned());
    }
    let url = reqwest::Url::parse(&config.control_server_url).map_err(|_| {
        "Enter a complete server address, such as https://remote.example.com.".to_owned()
    })?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() != "https" && !(cfg!(debug_assertions) && url.scheme() == "http" && loopback) {
        return Err(
            "The server must use HTTPS. HTTP is available only for local development builds."
                .to_owned(),
        );
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(
            "The server address cannot include credentials, a query, or a fragment.".to_owned(),
        );
    }
    config.relay_address = config.relay_address.trim().to_owned();
    config.relay_server_name = config.relay_server_name.trim().to_owned();
    config.ca_certificate_path = config.ca_certificate_path.trim().to_owned();
    config.stun_address = config.stun_address.trim().to_owned();
    if !config.stun_address.is_empty() {
        config
            .stun_address
            .parse::<std::net::SocketAddr>()
            .map_err(|_| {
                "The optional STUN address must use a numeric IP and UDP port.".to_owned()
            })?;
    }
    Ok(())
}

fn normalize_url(value: &str) -> String {
    remotex_control_client::normalize_control_url(value)
}

fn managed_default_config(app: &AppHandle) -> Result<ServerConfig, String> {
    complete_managed_defaults(app, ServerConfig::default())
}

fn complete_managed_defaults(
    app: &AppHandle,
    mut config: ServerConfig,
) -> Result<ServerConfig, String> {
    config.control_server_url = normalize_url(&config.control_server_url);
    if normalize_url(&config.control_server_url) != DEFAULT_CONTROL_SERVER_URL {
        return Ok(config);
    }
    if config.relay_address.trim().is_empty() {
        DEFAULT_RELAY_ADDRESS.clone_into(&mut config.relay_address);
    }
    if config.relay_server_name.trim().is_empty() {
        DEFAULT_RELAY_SERVER_NAME.clone_into(&mut config.relay_server_name);
    }
    let configured_certificate = PathBuf::from(config.ca_certificate_path.trim());
    if config.ca_certificate_path.trim().is_empty() || !configured_certificate.is_file() {
        config.ca_certificate_path = install_builtin_relay_ca(app)?.display().to_string();
    }
    normalize_and_validate(&mut config)?;
    config.configured = true;
    Ok(config)
}

fn install_builtin_relay_ca(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app
        .path()
        .app_local_data_dir()
        .map_err(|error| format!("resolve RemoteX data directory: {error}"))?
        .join("certificates");
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("create RemoteX certificate directory: {error}"))?;
    let path = directory.join("isrg-root-x1.pem");
    let current = std::fs::read(&path).ok();
    if current.as_deref() != Some(BUILTIN_RELAY_CA_CERTIFICATE) {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .map_err(|error| format!("open built-in Relay CA certificate: {error}"))?;
        file.write_all(BUILTIN_RELAY_CA_CERTIFICATE)
            .and_then(|()| file.sync_all())
            .map_err(|error| format!("install built-in Relay CA certificate: {error}"))?;
    }
    Ok(path)
}

async fn check_relay_identity(config: &ServerConfig) -> Result<(), String> {
    let relay_address = resolve_relay_address(&config.relay_address).await?;
    let endpoint = crate::client_endpoint(std::path::Path::new(&config.ca_certificate_path))
        .map_err(|error| format!("prepare Relay TLS verification: {error:#}"))?;
    let connecting = endpoint
        .connect(relay_address, &config.relay_server_name)
        .map_err(|error| format!("prepare Relay TLS connection: {error}"))?;
    let connection = tokio::time::timeout(std::time::Duration::from_secs(8), connecting)
        .await
        .map_err(|_| "Relay TLS verification timed out".to_owned())?
        .map_err(|error| format!("verify Relay TLS identity: {error}"))?;
    connection.close(0_u32.into(), b"RemoteX server configuration verified");
    endpoint.wait_idle().await;
    Ok(())
}

async fn resolve_relay_address(value: &str) -> Result<SocketAddr, String> {
    if let Some(address) = remotex_control_client::known_relay_address(value) {
        return Ok(address);
    }
    if let Ok(address) = value.parse() {
        return Ok(address);
    }
    tokio::net::lookup_host(value)
        .await
        .map_err(|error| format!("resolve Relay address: {error}"))?
        .find(SocketAddr::is_ipv4)
        .ok_or_else(|| "The Relay address did not resolve to an IPv4 address.".to_owned())
}

fn server_config_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map(|directory| directory.join("server-config.json"))
        .map_err(|error| format!("resolve RemoteX data directory: {error}"))
}

fn write_server_config(app: &AppHandle, config: &ServerConfig) -> Result<(), String> {
    let path = server_config_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create RemoteX data directory: {error}"))?;
    }
    let data = serde_json::to_vec_pretty(config)
        .map_err(|error| format!("encode server configuration: {error}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("open server configuration: {error}"))?;
    file.write_all(&data)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write server configuration: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_install_uses_the_managed_control_server_by_default() {
        let config = ServerConfig::default();

        assert_eq!(config.control_server_url, DEFAULT_CONTROL_SERVER_URL);
        assert_eq!(config.relay_address, DEFAULT_RELAY_ADDRESS);
        assert_eq!(config.relay_server_name, DEFAULT_RELAY_SERVER_NAME);
        assert!(!config.configured);
    }

    #[test]
    fn production_style_server_validation_requires_https() {
        let mut secure = ServerConfig {
            control_server_url: "https://remote.example.com/".to_owned(),
            ca_certificate_path: "C:\\RemoteX\\ca.pem".to_owned(),
            ..ServerConfig::default()
        };
        assert!(normalize_and_validate(&mut secure).is_ok());
        assert_eq!(secure.control_server_url, "https://remote.example.com");

        let mut insecure = ServerConfig {
            control_server_url: "http://remote.example.com".to_owned(),
            ca_certificate_path: "C:\\RemoteX\\ca.pem".to_owned(),
            ..ServerConfig::default()
        };
        assert!(normalize_and_validate(&mut insecure).is_err());
    }

    #[test]
    fn placeholder_is_not_migrated_as_a_custom_server() {
        assert!(
            candidate_from_parts("This device", PLACEHOLDER_CONTROL_URL, "", "", "ca.pem", "")
                .is_none()
        );
    }

    #[test]
    fn unconfigured_managed_default_is_not_treated_as_a_migration_candidate() {
        assert!(
            candidate_from_parts("This device", DEFAULT_CONTROL_SERVER_URL, "", "", "", "")
                .is_none()
        );
    }

    #[tokio::test]
    #[ignore = "requires the deployed RemoteX Relay"]
    async fn deployed_relay_presents_an_identity_trusted_by_the_builtin_ca() {
        let certificate_path =
            std::env::temp_dir().join(format!("remotex-isrg-root-x1-{}.pem", std::process::id()));
        std::fs::write(&certificate_path, BUILTIN_RELAY_CA_CERTIFICATE)
            .expect("write temporary built-in CA certificate");
        let config = ServerConfig {
            ca_certificate_path: certificate_path.display().to_string(),
            ..ServerConfig::default()
        };

        let result = check_relay_identity(&config).await;
        let _remove_result = std::fs::remove_file(certificate_path);
        result.expect("deployed Relay TLS identity must verify");
    }

    #[tokio::test]
    #[ignore = "requires the deployed RemoteX Control and Relay"]
    async fn deployed_managed_server_passes_the_complete_client_check() {
        let certificate_path = std::env::temp_dir().join(format!(
            "remotex-managed-check-isrg-root-x1-{}.pem",
            std::process::id()
        ));
        std::fs::write(&certificate_path, BUILTIN_RELAY_CA_CERTIFICATE)
            .expect("write temporary built-in CA certificate");
        let config = ServerConfig {
            ca_certificate_path: certificate_path.display().to_string(),
            ..ServerConfig::default()
        };

        let result = check_server_connection(config).await;
        let _remove_result = std::fs::remove_file(certificate_path);
        result
            .expect("deployed managed server check must complete")
            .ok
            .then_some(())
            .expect("deployed managed server must be ready");
    }
}
