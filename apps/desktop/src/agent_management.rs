#![allow(unsafe_code)]
#![allow(clippy::needless_pass_by_value)]

use base64::{Engine, engine::general_purpose::STANDARD};
use rand::TryRngCore;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_shell::{
    ShellExt,
    process::{CommandChild, CommandEvent},
};

#[cfg(windows)]
use windows::{
    Win32::{
        Foundation::{HLOCAL, LocalFree},
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    },
    core::w,
};

const MAX_SETTINGS_SIZE: u64 = 64 * 1024;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum VideoQuality {
    #[default]
    Auto,
    #[serde(alias = "high")]
    Quality,
    Balanced,
    #[serde(alias = "low")]
    LowBandwidth,
}

impl VideoQuality {
    const fn frames_per_second(self) -> u32 {
        match self {
            Self::Auto | Self::Quality => 30,
            Self::Balanced => 20,
            Self::LowBandwidth => 10,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct AgentSettings {
    pub remote_access_enabled: bool,
    pub server_url: String,
    pub device_name: String,
    pub ca_certificate_path: String,
    pub allow_input: bool,
    pub allow_clipboard: bool,
    pub allow_file_upload: bool,
    pub allow_file_download: bool,
    pub file_roots: String,
    pub unattended_access: bool,
    #[serde(skip_serializing)]
    pub unattended_secret: String,
    pub secret_configured: bool,
    pub start_with_windows: bool,
    pub video_quality: VideoQuality,
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            remote_access_enabled: false,
            server_url: crate::server_config::DEFAULT_CONTROL_SERVER_URL.to_owned(),
            device_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".to_owned()),
            ca_certificate_path: String::new(),
            allow_input: false,
            allow_clipboard: false,
            allow_file_upload: false,
            allow_file_download: false,
            file_roots: String::new(),
            unattended_access: false,
            unattended_secret: String::new(),
            secret_configured: false,
            start_with_windows: false,
            video_quality: VideoQuality::Auto,
        }
    }
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct StoredAgentSettings {
    #[serde(flatten)]
    public: AgentSettings,
    unattended_secret_protected: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct AgentStatusFile {
    device_id: Option<String>,
    state: String,
    session_id: Option<String>,
    updated_at_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeStatus {
    pub running: bool,
    pub process_id: Option<u32>,
    pub device_id: Option<String>,
    pub state: String,
    pub session_id: Option<String>,
    pub start_with_windows: bool,
}

pub(super) struct LegacyServerConfig {
    pub server_url: String,
    pub ca_certificate_path: String,
}

#[derive(Default)]
pub struct AgentRuntime {
    child: Mutex<Option<CommandChild>>,
}

#[tauri::command]
pub fn load_agent_settings(app: AppHandle) -> Result<AgentSettings, String> {
    let mut stored = read_settings(&app)?;
    stored.public.unattended_secret.clear();
    stored.public.secret_configured = stored.unattended_secret_protected.is_some();
    stored.public.start_with_windows = app
        .autolaunch()
        .is_enabled()
        .map_err(|error| format!("read Windows startup setting: {error}"))?;
    Ok(stored.public)
}

#[tauri::command]
pub fn save_agent_settings(app: AppHandle, mut settings: AgentSettings) -> Result<(), String> {
    let existing = read_settings(&app)?;
    ensure_access_password(
        &mut settings,
        existing.unattended_secret_protected.is_some(),
    )?;
    validate_settings(&settings, existing.unattended_secret_protected.is_some())?;
    let protected = if settings.unattended_secret.is_empty() {
        existing.unattended_secret_protected
    } else {
        Some(protect_secret(&settings.unattended_secret)?)
    };
    settings.unattended_secret.clear();
    settings.secret_configured = protected.is_some();
    let runtime = app.state::<AgentRuntime>();
    let was_running = runtime
        .child
        .lock()
        .map_err(|_| "Agent process lock is unavailable".to_owned())?
        .is_some();
    if was_running && settings.remote_access_enabled && read_status_file(&app)?.session_id.is_some()
    {
        return Err(
            "Disconnect the active incoming session before changing access settings.".to_owned(),
        );
    }
    configure_startup(&app, settings.start_with_windows)?;
    if was_running {
        stop_agent_inner(&runtime)?;
    }
    let keep_running = was_running && settings.remote_access_enabled;
    let result = write_settings(
        &app,
        &StoredAgentSettings {
            public: settings,
            unattended_secret_protected: protected,
        },
    );
    if result.is_err() {
        if was_running {
            let _restart = start_agent_inner(&app, &runtime);
        }
        return result;
    }
    if keep_running {
        start_agent_inner(&app, &runtime)?;
    }
    Ok(())
}

fn validate_access_password(password: &str) -> Result<(), String> {
    if !(12..=128).contains(&password.len()) || password.chars().any(char::is_control) {
        return Err(
            "Use a connection password of 12 to 128 bytes without control characters.".to_owned(),
        );
    }
    Ok(())
}

fn random_access_password() -> Result<String, String> {
    // 16 independent base-32 characters = 80 bits from the OS CSPRNG.
    const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0_u8; 16];
    rand::rngs::OsRng
        .try_fill_bytes(&mut bytes)
        .map_err(|_| "Could not securely generate a connection password.".to_owned())?;
    Ok(bytes
        .iter()
        .map(|byte| char::from(ALPHABET[usize::from(byte & 31)]))
        .collect())
}

fn ensure_access_password(
    settings: &mut AgentSettings,
    existing_secret: bool,
) -> Result<(), String> {
    if settings.remote_access_enabled && !existing_secret && settings.unattended_secret.is_empty() {
        settings.unattended_secret = random_access_password()?;
        settings.unattended_access = true;
    }
    if !settings.unattended_secret.is_empty() {
        validate_access_password(&settings.unattended_secret)?;
    }
    Ok(())
}

/// Only an explicit local reveal/copy action returns plaintext to the `WebView`.
#[tauri::command]
pub fn reveal_access_password(app: AppHandle) -> Result<Option<String>, String> {
    read_settings(&app)?
        .unattended_secret_protected
        .as_deref()
        .map(unprotect_secret)
        .transpose()
}

#[tauri::command]
pub fn update_access_password(app: AppHandle, password: Option<String>) -> Result<(), String> {
    let mut settings = load_agent_settings(app.clone())?;
    settings.unattended_secret = password.map_or_else(random_access_password, Ok)?;
    validate_access_password(&settings.unattended_secret)?;
    save_agent_settings(app, settings)
}

#[tauri::command]
pub fn start_agent(
    app: AppHandle,
    runtime: State<'_, AgentRuntime>,
) -> Result<AgentRuntimeStatus, String> {
    start_agent_inner(&app, &runtime)?;
    agent_status_inner(&app, &runtime)
}

#[tauri::command]
pub fn stop_agent(
    app: AppHandle,
    runtime: State<'_, AgentRuntime>,
) -> Result<AgentRuntimeStatus, String> {
    stop_agent_inner(&runtime)?;
    agent_status_inner(&app, &runtime)
}

#[tauri::command]
pub fn agent_status(
    app: AppHandle,
    runtime: State<'_, AgentRuntime>,
) -> Result<AgentRuntimeStatus, String> {
    agent_status_inner(&app, &runtime)
}

pub fn start_configured_agent(app: &AppHandle) -> Result<(), String> {
    let runtime = app.state::<AgentRuntime>();
    let settings = read_settings(app)?;
    if settings.public.remote_access_enabled {
        start_agent_inner(app, &runtime)?;
    }
    Ok(())
}

pub fn stop_configured_agent(app: &AppHandle) -> Result<(), String> {
    stop_agent_inner(&app.state::<AgentRuntime>())
}

pub fn runtime_status(app: &AppHandle) -> Result<AgentRuntimeStatus, String> {
    agent_status_inner(app, &app.state::<AgentRuntime>())
}

pub fn disable_remote_access(app: &AppHandle) -> Result<(), String> {
    stop_configured_agent(app)?;
    let mut stored = read_settings(app)?;
    stored.public.remote_access_enabled = false;
    write_settings(app, &stored)
}

fn start_agent_inner(app: &AppHandle, runtime: &AgentRuntime) -> Result<(), String> {
    let mut guard = runtime
        .child
        .lock()
        .map_err(|_| "Agent process lock is unavailable".to_owned())?;
    if guard.is_some() {
        return Ok(());
    }
    let stored = read_settings(app)?;
    validate_settings(&stored.public, stored.unattended_secret_protected.is_some())?;
    if !stored.public.remote_access_enabled {
        return Err("enable Remote Access in Settings before starting the Agent".to_owned());
    }
    let environment = agent_environment(app, &stored)?;
    let (mut events, child) = app
        .shell()
        .sidecar("remotex-agent")
        .map_err(|error| format!("locate packaged RemoteX Agent: {error}"))?
        .envs(environment)
        .spawn()
        .map_err(|error| format!("start packaged RemoteX Agent: {error}"))?;
    let process_id = child.pid();
    *guard = Some(child);
    drop(guard);

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = events.recv().await {
            if matches!(event, CommandEvent::Terminated(_)) {
                if let Ok(mut child) = handle.state::<AgentRuntime>().child.lock()
                    && child
                        .as_ref()
                        .is_some_and(|active| active.pid() == process_id)
                {
                    child.take();
                }
                break;
            }
        }
    });
    Ok(())
}

fn stop_agent_inner(runtime: &AgentRuntime) -> Result<(), String> {
    let child = runtime
        .child
        .lock()
        .map_err(|_| "Agent process lock is unavailable".to_owned())?
        .take();
    if let Some(child) = child {
        child
            .kill()
            .map_err(|error| format!("stop RemoteX Agent: {error}"))?;
    }
    Ok(())
}

fn agent_status_inner(
    app: &AppHandle,
    runtime: &AgentRuntime,
) -> Result<AgentRuntimeStatus, String> {
    let process_id = runtime
        .child
        .lock()
        .map_err(|_| "Agent process lock is unavailable".to_owned())?
        .as_ref()
        .map(CommandChild::pid);
    let status = read_status_file(app).unwrap_or_default();
    Ok(AgentRuntimeStatus {
        running: process_id.is_some(),
        process_id,
        device_id: status.device_id,
        state: if process_id.is_some() {
            if status.state.is_empty() {
                "starting".to_owned()
            } else {
                status.state
            }
        } else {
            "offline".to_owned()
        },
        session_id: status.session_id,
        start_with_windows: app
            .autolaunch()
            .is_enabled()
            .map_err(|error| format!("read Windows startup setting: {error}"))?,
    })
}

fn agent_environment(
    app: &AppHandle,
    stored: &StoredAgentSettings,
) -> Result<HashMap<String, String>, String> {
    let settings = &stored.public;
    let unified = crate::server_config::read_server_config(app)?;
    let server_url = unified
        .as_ref()
        .filter(|config| config.configured)
        .map_or_else(
            || settings.server_url.clone(),
            |config| config.control_server_url.clone(),
        );
    let ca_certificate_path = unified
        .as_ref()
        .filter(|config| config.configured)
        .map_or_else(
            || settings.ca_certificate_path.clone(),
            |config| config.ca_certificate_path.clone(),
        );
    let data_directory = data_directory(app)?;
    let mut environment = HashMap::from([
        ("REMOTEX_CONTROL_URL".to_owned(), server_url),
        (
            "REMOTEX_IDENTITY_PATH".to_owned(),
            data_directory.join("identity.json").display().to_string(),
        ),
        (
            "REMOTEX_STATUS_PATH".to_owned(),
            status_path(app)?.display().to_string(),
        ),
        ("REMOTEX_RELAY_CA_CERT".to_owned(), ca_certificate_path),
        (
            "REMOTEX_DEVICE_NAME".to_owned(),
            settings.device_name.clone(),
        ),
        (
            "REMOTEX_VIDEO_FPS".to_owned(),
            settings.video_quality.frames_per_second().to_string(),
        ),
        (
            "REMOTEX_VIDEO_PROFILE".to_owned(),
            serde_json::to_value(settings.video_quality)
                .ok()
                .and_then(|value| value.as_str().map(ToOwned::to_owned))
                .unwrap_or_else(|| "auto".to_owned()),
        ),
        (
            "REMOTEX_ALLOW_INPUT".to_owned(),
            settings.allow_input.to_string(),
        ),
        (
            "REMOTEX_ALLOW_CLIPBOARD".to_owned(),
            settings.allow_clipboard.to_string(),
        ),
        (
            "REMOTEX_ALLOW_FILE_UPLOAD".to_owned(),
            settings.allow_file_upload.to_string(),
        ),
        (
            "REMOTEX_ALLOW_FILE_DOWNLOAD".to_owned(),
            settings.allow_file_download.to_string(),
        ),
        ("REMOTEX_FILE_ROOTS".to_owned(), settings.file_roots.clone()),
        (
            "REMOTEX_UNATTENDED_ACCESS".to_owned(),
            settings.unattended_access.to_string(),
        ),
    ]);
    if let Some(stun_address) = unified
        .as_ref()
        .map(|config| config.stun_address.trim())
        .filter(|address| !address.is_empty())
    {
        environment.insert("REMOTEX_STUN_ADDRESS".to_owned(), stun_address.to_owned());
    }
    if let Some(secret) = stored.unattended_secret_protected.as_deref() {
        environment.insert(
            "REMOTEX_UNATTENDED_SECRET".to_owned(),
            unprotect_secret(secret)?,
        );
    }
    Ok(environment)
}

pub(super) fn legacy_server_config(app: &AppHandle) -> Result<LegacyServerConfig, String> {
    let stored = read_settings(app)?;
    Ok(LegacyServerConfig {
        server_url: stored.public.server_url,
        ca_certificate_path: stored.public.ca_certificate_path,
    })
}

pub(super) fn sync_server_fields(
    app: &AppHandle,
    config: &crate::server_config::ServerConfig,
) -> Result<(), String> {
    let mut stored = read_settings(app)?;
    stored
        .public
        .server_url
        .clone_from(&config.control_server_url);
    stored
        .public
        .ca_certificate_path
        .clone_from(&config.ca_certificate_path);
    write_settings(app, &stored)
}

fn validate_settings(settings: &AgentSettings, existing_secret: bool) -> Result<(), String> {
    if !settings.unattended_secret.is_empty() {
        validate_access_password(&settings.unattended_secret)?;
    }
    if !settings.remote_access_enabled {
        return Ok(());
    }
    let url = reqwest::Url::parse(&settings.server_url)
        .map_err(|_| "Control Server URL is invalid".to_owned())?;
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        return Err(
            "Control Server must use HTTPS (HTTP is allowed only for loopback development)"
                .to_owned(),
        );
    }
    if settings.server_url.len() > 2_048 {
        return Err("Control Server URL is too long".to_owned());
    }
    if settings.device_name.is_empty()
        || settings.device_name.len() > 128
        || settings.device_name.chars().any(char::is_control)
    {
        return Err("Device Name must contain 1 to 128 visible characters".to_owned());
    }
    if settings.ca_certificate_path.is_empty() || settings.ca_certificate_path.len() > 4_096 {
        return Err("Relay CA certificate path is required".to_owned());
    }
    if settings.file_roots.len() > 16 * 1024 {
        return Err("Allowed file roots are too long".to_owned());
    }
    if (settings.allow_file_upload || settings.allow_file_download)
        && settings.file_roots.trim().is_empty()
    {
        return Err("File permissions require at least one Name=Path allowed root".to_owned());
    }
    if settings.unattended_access
        && !existing_secret
        && !(12..=128).contains(&settings.unattended_secret.len())
    {
        return Err("Unattended Access requires a new 12 to 128 byte secret".to_owned());
    }
    if !settings.unattended_secret.is_empty()
        && !(12..=128).contains(&settings.unattended_secret.len())
    {
        return Err("Unattended Access secret must contain 12 to 128 bytes".to_owned());
    }
    Ok(())
}

fn configure_startup(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    configure_startup_with(
        enabled,
        || {
            manager
                .is_enabled()
                .map_err(|error| format!("read Windows startup setting: {error}"))
        },
        |enabled| {
            if enabled {
                manager
                    .enable()
                    .map_err(|error| format!("enable Start with Windows: {error}"))
            } else {
                manager
                    .disable()
                    .map_err(|error| format!("disable Start with Windows: {error}"))
            }
        },
    )
}

fn configure_startup_with(
    enabled: bool,
    read_enabled: impl FnOnce() -> Result<bool, String>,
    write_enabled: impl FnOnce(bool) -> Result<(), String>,
) -> Result<(), String> {
    // auto-launch deletes the Windows Run value unconditionally. A fresh
    // installation has no value to delete: saving unrelated Agent settings
    // must not fail just because Start with Windows is already disabled.
    if !enabled && !read_enabled()? {
        return Ok(());
    }
    // Keep enable() even when already enabled, so the executable path is
    // refreshed after moving/upgrading an installation. Do not swallow errors.
    write_enabled(enabled)
}

fn data_directory(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_local_data_dir()
        .map_err(|error| format!("resolve RemoteX data directory: {error}"))
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_directory(app)?.join("agent-settings.json"))
}

fn status_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_directory(app)?.join("agent-status.json"))
}

fn read_settings(app: &AppHandle) -> Result<StoredAgentSettings, String> {
    let path = settings_path(app)?;
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.len() > MAX_SETTINGS_SIZE => {
            return Err("Agent Settings file exceeds 64 KiB".to_owned());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(StoredAgentSettings::default());
        }
        Err(error) => return Err(format!("inspect Agent Settings: {error}")),
    }
    let data = std::fs::read(&path).map_err(|error| format!("read Agent Settings: {error}"))?;
    serde_json::from_slice(&data).map_err(|error| format!("decode Agent Settings: {error}"))
}

fn write_settings(app: &AppHandle, stored: &StoredAgentSettings) -> Result<(), String> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("create RemoteX data directory: {error}"))?;
    }
    let data = serde_json::to_vec_pretty(stored)
        .map_err(|error| format!("encode Agent Settings: {error}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&path)
        .map_err(|error| format!("open Agent Settings: {error}"))?;
    file.write_all(&data)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write Agent Settings: {error}"))
}

fn read_status_file(app: &AppHandle) -> Result<AgentStatusFile, String> {
    let path = status_path(app)?;
    read_bounded_json(&path, 16 * 1024)
}

fn read_bounded_json<T: serde::de::DeserializeOwned>(path: &Path, limit: u64) -> Result<T, String> {
    let metadata = std::fs::metadata(path).map_err(|error| format!("inspect status: {error}"))?;
    if metadata.len() > limit {
        return Err("Agent status file is too large".to_owned());
    }
    let data = std::fs::read(path).map_err(|error| format!("read Agent status: {error}"))?;
    serde_json::from_slice(&data).map_err(|error| format!("decode Agent status: {error}"))
}

#[cfg(windows)]
fn protect_secret(secret: &str) -> Result<String, String> {
    let input = blob(secret.as_bytes())?;
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: Input references the live UTF-8 secret buffer. Output is initialized by DPAPI,
    // copied immediately, and released with LocalFree. UI is explicitly forbidden.
    unsafe {
        CryptProtectData(
            &raw const input,
            w!("RemoteX unattended access secret"),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &raw mut output,
        )
        .map_err(|error| format!("protect unattended secret with Windows DPAPI: {error}"))?;
    }
    copy_and_free(output).map(|bytes| STANDARD.encode(bytes))
}

#[cfg(windows)]
fn unprotect_secret(protected: &str) -> Result<String, String> {
    let encrypted = STANDARD
        .decode(protected)
        .map_err(|_| "stored unattended secret is malformed".to_owned())?;
    let input = blob(&encrypted)?;
    let mut output = CRYPT_INTEGER_BLOB::default();
    // SAFETY: Input references the live decoded buffer. DPAPI allocates output for this user;
    // it is copied immediately and released with LocalFree. UI is explicitly forbidden.
    unsafe {
        CryptUnprotectData(
            &raw const input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &raw mut output,
        )
        .map_err(|error| format!("unprotect unattended secret with Windows DPAPI: {error}"))?;
    }
    String::from_utf8(copy_and_free(output)?)
        .map_err(|_| "stored unattended secret is not UTF-8".to_owned())
}

#[cfg(windows)]
fn blob(data: &[u8]) -> Result<CRYPT_INTEGER_BLOB, String> {
    Ok(CRYPT_INTEGER_BLOB {
        cbData: data
            .len()
            .try_into()
            .map_err(|_| "secret exceeds Windows DPAPI input limit".to_owned())?,
        pbData: data.as_ptr().cast_mut(),
    })
}

#[cfg(windows)]
fn copy_and_free(output: CRYPT_INTEGER_BLOB) -> Result<Vec<u8>, String> {
    if output.pbData.is_null() || output.cbData == 0 {
        return Err("Windows DPAPI returned empty output".to_owned());
    }
    // SAFETY: DPAPI returned a valid buffer of cbData bytes. We copy before LocalFree and never
    // access the allocation again.
    let bytes = unsafe {
        let value = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _result = LocalFree(Some(HLOCAL(output.pbData.cast())));
        value
    };
    Ok(bytes)
}

#[cfg(not(windows))]
fn protect_secret(_secret: &str) -> Result<String, String> {
    Err("unattended secret storage is supported only on Windows".to_owned())
}

#[cfg(not(windows))]
fn unprotect_secret(_protected: &str) -> Result<String, String> {
    Err("unattended secret storage is supported only on Windows".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_activation_creates_a_password_without_changing_permissions() {
        let mut settings = AgentSettings {
            remote_access_enabled: true,
            ..AgentSettings::default()
        };
        ensure_access_password(&mut settings, false).unwrap();
        assert!(settings.unattended_access);
        assert_eq!(settings.unattended_secret.len(), 16);
        assert!(!settings.allow_input && !settings.allow_file_upload && !settings.allow_clipboard);
        assert!(
            !serde_json::to_string(&settings)
                .unwrap()
                .contains(&settings.unattended_secret)
        );
    }

    #[test]
    fn existing_password_and_approval_only_choice_are_preserved() {
        let mut settings = AgentSettings {
            remote_access_enabled: true,
            ..AgentSettings::default()
        };
        ensure_access_password(&mut settings, true).unwrap();
        assert!(!settings.unattended_access);
        assert!(settings.unattended_secret.is_empty());
        let mut disabled = AgentSettings::default();
        ensure_access_password(&mut disabled, false).unwrap();
        assert!(disabled.unattended_secret.is_empty());
    }

    #[test]
    fn generated_passwords_are_readable_and_custom_passwords_are_validated() {
        let values = (0..128)
            .map(|_| random_access_password().unwrap())
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(values.len(), 128);
        for value in values {
            assert_eq!(value.len(), 16);
            validate_access_password(&value).unwrap();
        }
        assert!(validate_access_password("correct horse battery").is_ok());
        assert!(validate_access_password("short").is_err());
        assert!(validate_access_password("correct\npassword").is_err());
        assert!(validate_access_password(&"a".repeat(129)).is_err());
        assert!(validate_access_password(&"a".repeat(128)).is_ok());
    }

    #[test]
    fn saving_with_startup_already_disabled_does_not_delete_a_missing_entry() {
        for _ in 0..3 {
            assert!(
                configure_startup_with(
                    false,
                    || Ok(false),
                    |_| panic!("must not delete an absent startup entry"),
                )
                .is_ok()
            );
        }
    }

    #[test]
    fn disabling_enabled_startup_writes_the_requested_state() {
        let mut written = None;
        configure_startup_with(
            false,
            || Ok(true),
            |enabled| {
                written = Some(enabled);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(written, Some(false));
    }

    #[test]
    fn enabling_startup_refreshes_the_entry() {
        let mut written = None;
        configure_startup_with(
            true,
            || panic!("enable does not need a read"),
            |enabled| {
                written = Some(enabled);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(written, Some(true));
    }

    #[test]
    fn startup_read_and_write_failures_are_not_hidden() {
        let error = "access denied";
        assert_eq!(
            configure_startup_with(false, || Err(error.to_owned()), |_| panic!("read failed")),
            Err(error.to_owned()),
        );
        for enabled in [false, true] {
            assert_eq!(
                configure_startup_with(enabled, || Ok(true), |_| Err(error.to_owned())),
                Err(error.to_owned()),
            );
        }
    }

    #[test]
    fn new_install_uses_the_managed_control_server_by_default() {
        assert_eq!(
            AgentSettings::default().server_url,
            crate::server_config::DEFAULT_CONTROL_SERVER_URL
        );
    }

    #[test]
    fn disabled_agent_does_not_require_server_settings() {
        assert!(validate_settings(&AgentSettings::default(), false).is_ok());
    }

    #[test]
    fn active_agent_requires_tls_and_explicit_file_roots() {
        let mut settings = AgentSettings {
            remote_access_enabled: true,
            server_url: "http://control.example.com".to_owned(),
            ca_certificate_path: "C:\\RemoteX\\ca.pem".to_owned(),
            ..AgentSettings::default()
        };
        assert!(validate_settings(&settings, false).is_err());
        settings.server_url = "https://control.example.com".to_owned();
        settings.allow_file_download = true;
        assert!(validate_settings(&settings, false).is_err());
        settings.file_roots = "Documents=C:\\Users\\User\\Documents".to_owned();
        assert!(validate_settings(&settings, false).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_secret_round_trips_for_current_windows_user() {
        let protected = protect_secret("correct horse battery staple").expect("protect");
        assert!(!protected.contains("correct horse"));
        assert_eq!(
            unprotect_secret(&protected).expect("unprotect"),
            "correct horse battery staple"
        );
    }
}
