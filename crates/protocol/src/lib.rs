//! Versioned, platform-neutral `RemoteX` protocol domain types.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{fmt, str::FromStr};
use thiserror::Error;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const DEFAULT_FILE_CHUNK_SIZE: u32 = 4 * 1024 * 1024;
pub const MAX_RELAY_HANDSHAKE_SIZE: usize = 4 * 1024;
pub const MAX_CLIPBOARD_TEXT_SIZE: usize = 1024 * 1024;
pub const MAX_FILE_CHUNK_SIZE: u32 = 4 * 1024 * 1024;
pub const MAX_FILE_PATH_SIZE: usize = 4 * 1024;
pub const MAX_DIRECTORY_ENTRIES: usize = 10_000;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(Uuid);

impl SessionId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
    #[must_use]
    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }
    #[must_use]
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
    }
}
impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl FromStr for SessionId {
    type Err = uuid::Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        if value.len() != 9 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(ProtocolError::InvalidDeviceId);
        }
        Ok(Self(value))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl TryFrom<String> for DeviceId {
    type Error = ProtocolError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}
impl From<DeviceId> for String {
    fn from(value: DeviceId) -> Self {
        value.0
    }
}
impl FromStr for DeviceId {
    type Err = ProtocolError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

/// Stable display identifier carried by input events for future multi-monitor UI.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DisplayId(String);

impl DisplayId {
    pub fn new(value: impl Into<String>) -> Result<Self, ProtocolError> {
        let value = value.into();
        if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
            return Err(ProtocolError::InvalidDisplayId);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DisplayId {
    type Error = ProtocolError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<DisplayId> for String {
    fn from(value: DisplayId) -> Self {
        value.0
    }
}

impl fmt::Display for DisplayId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for DisplayId {
    type Err = ProtocolError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferId(Uuid);

impl TransferId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    #[must_use]
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}
impl Default for TransferId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TransferId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for TransferId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

/// Opaque 256-bit bearer credential presented exactly once to the relay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionToken([u8; 32]);

impl SessionToken {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Channel {
    Control = 0,
    Video = 1,
    Input = 2,
    Clipboard = 3,
    FileTransfer = 4,
    Audio = 5,
    Telemetry = 6,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum Role {
    Controller,
    Agent,
}

/// Authentication message sent in the first relay frame on a QUIC stream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClientHello {
    pub version: u16,
    pub session_id: SessionId,
    pub role: Role,
    pub token: SessionToken,
}

impl ClientHello {
    #[must_use]
    pub const fn new(session_id: SessionId, role: Role, token: SessionToken) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            session_id,
            role,
            token,
        }
    }
}

/// Compatibility name retained for the M3 composition roots.
pub type RelayHandshake = ClientHello;

/// Messages sent from an authenticated peer to the relay.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RelayClientMessage {
    ClientHello(ClientHello),
    Payload(Vec<u8>),
    Heartbeat { nonce: u64 },
    HeartbeatAck { nonce: u64 },
    Close,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RelayProtocolErrorCode {
    UnsupportedVersion,
    UnknownSession,
    ExpiredSession,
    InvalidToken,
    RoleMismatch,
    TokenAlreadyUsed,
    DuplicateRole,
    SessionNotReady,
    MalformedMessage,
    FrameTooLarge,
    CapacityExceeded,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SessionCloseReason {
    ClientClosed,
    PeerDisconnected,
    HeartbeatTimeout,
    ProtocolViolation,
    SlowConsumer,
    RelayShutdown,
}

/// Relay control messages and opaque application payload delivery.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RelayServerMessage {
    WaitingForPeer {
        role: Role,
    },
    PeerReady,
    Payload(Vec<u8>),
    Heartbeat {
        nonce: u64,
    },
    HeartbeatAck {
        nonce: u64,
    },
    SessionClosed {
        reason: SessionCloseReason,
    },
    ProtocolError {
        code: RelayProtocolErrorCode,
        message: String,
    },
}

/// Serializes a protocol value using the single workspace wire codec.
pub fn encode_wire<T: Serialize>(value: &T) -> Result<Vec<u8>, ProtocolCodecError> {
    bincode::serde::encode_to_vec(value, bincode::config::standard())
        .map_err(|error| ProtocolCodecError::Encode(error.to_string()))
}

/// Deserializes one complete protocol value and rejects trailing bytes.
pub fn decode_wire<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ProtocolCodecError> {
    let (value, consumed) = bincode::serde::decode_from_slice(bytes, bincode::config::standard())
        .map_err(|error| ProtocolCodecError::Decode(error.to_string()))?;
    if consumed != bytes.len() {
        return Err(ProtocolCodecError::TrailingBytes);
    }
    Ok(value)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct SessionPermissions {
    pub view_desktop: bool,
    pub control_input: bool,
    pub clipboard: bool,
    pub file_upload: bool,
    pub file_download: bool,
}

impl SessionPermissions {
    #[must_use]
    pub const fn intersect(self, available: Self) -> Self {
        Self {
            view_desktop: self.view_desktop && available.view_desktop,
            control_input: self.control_input && available.control_input,
            clipboard: self.clipboard && available.clipboard,
            file_upload: self.file_upload && available.file_upload,
            file_download: self.file_download && available.file_download,
        }
    }

    #[must_use]
    pub const fn is_subset_of(self, available: Self) -> bool {
        (!self.view_desktop || available.view_desktop)
            && (!self.control_input || available.control_input)
            && (!self.clipboard || available.clipboard)
            && (!self.file_upload || available.file_upload)
            && (!self.file_download || available.file_download)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DevicePlatform {
    Windows,
    Linux,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceRegistrationRequest {
    pub public_key: Vec<u8>,
    pub device_name: String,
    pub platform: DevicePlatform,
    pub agent_version: String,
    pub capabilities: SessionPermissions,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceRegistrationResponse {
    pub device_id: DeviceId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceAuthProof {
    pub timestamp_ms: u64,
    pub nonce: u64,
    pub signature: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceHeartbeatRequest {
    pub proof: DeviceAuthProof,
    pub agent_version: String,
    pub platform: DevicePlatform,
    pub capabilities: SessionPermissions,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub device_id: DeviceId,
    pub device_name: String,
    pub platform: DevicePlatform,
    pub agent_version: String,
    pub capabilities: SessionPermissions,
    pub last_seen_ms: u64,
    pub online: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    pub device_id: DeviceId,
    pub controller_name: String,
    pub requested_permissions: SessionPermissions,
    #[serde(default)]
    pub unattended_secret: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionCredentials {
    pub session_id: SessionId,
    pub relay_address: String,
    pub relay_server_name: String,
    pub role_token_hex: String,
    pub end_to_end_key_hex: String,
    pub expires_at_ms: u64,
    pub permissions: SessionPermissions,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClaimAgentSessionRequest {
    pub proof: DeviceAuthProof,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClaimAgentSessionResponse {
    pub authorization_request: Option<IncomingSessionRequest>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IncomingSessionRequest {
    pub session_id: SessionId,
    pub controller_name: String,
    pub requested_permissions: SessionPermissions,
    pub expires_at_ms: u64,
    pub unattended_secret: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationDecision {
    Accept,
    Reject,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolveSessionAuthorizationRequest {
    pub proof: DeviceAuthProof,
    pub decision: AuthorizationDecision,
    pub granted_permissions: SessionPermissions,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResolveSessionAuthorizationResponse {
    pub credentials: Option<SessionCredentials>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionType {
    Relay,
    Direct,
    Lan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionAuditEventKind {
    Started,
    Ended,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReportSessionEventRequest {
    pub proof: DeviceAuthProof,
    pub kind: SessionAuditEventKind,
    pub connection_type: ConnectionType,
    pub bytes_transferred: u64,
    pub result: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ControlMessage {
    SessionRequest { device_id: DeviceId },
    SessionAccepted { permissions: SessionPermissions },
    SessionRejected { reason: String },
    SessionEnded { reason: String },
    Ping { nonce: u64 },
    Pong { nonce: u64 },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ButtonState {
    Down,
    Up,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum WheelAxis {
    Vertical,
    Horizontal,
}

/// Platform-neutral physical key positions matching browser `KeyboardEvent.code` names.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum KeyCode {
    KeyA,
    KeyB,
    KeyC,
    KeyD,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyS,
    KeyT,
    KeyU,
    KeyV,
    KeyW,
    KeyX,
    KeyY,
    KeyZ,
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Space,
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    SuperLeft,
    SuperRight,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove {
        display_id: Option<DisplayId>,
        normalized_x: u16,
        normalized_y: u16,
    },
    MouseButtonDown {
        button: MouseButton,
    },
    MouseButtonUp {
        button: MouseButton,
    },
    MouseWheel {
        axis: WheelAxis,
        delta: i32,
    },
    KeyDown {
        key: KeyCode,
    },
    KeyUp {
        key: KeyCode,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum ClipboardOrigin {
    Controller,
    Agent,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ClipboardMessage {
    Text {
        origin: ClipboardOrigin,
        revision: u64,
        text: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum VideoCodec {
    Jpeg,
    WebP,
}

/// One independently decodable compressed desktop frame.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EncodedVideoFrame {
    pub width: u32,
    pub height: u32,
    pub source_timestamp_ms: u64,
    pub codec: VideoCodec,
    pub key_frame: bool,
    pub payload: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FileEntryKind {
    File,
    Directory,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub kind: FileEntryKind,
    pub size: u64,
    pub modified_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FileTransferDirection {
    Upload,
    Download,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FileTransferErrorCode {
    PermissionDenied,
    InvalidPath,
    NotFound,
    AlreadyExists,
    InvalidOffset,
    InvalidChunk,
    ChecksumMismatch,
    Cancelled,
    Busy,
    Io,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FileTransferMessage {
    ListDirectoryRequest {
        request_id: u64,
        path: String,
    },
    ListDirectoryResponse {
        request_id: u64,
        path: String,
        entries: Vec<FileEntry>,
    },
    CreateDirectoryRequest {
        request_id: u64,
        path: String,
    },
    CreateDirectoryResponse {
        request_id: u64,
        path: String,
    },
    DownloadRequest {
        transfer_id: TransferId,
        source_path: String,
    },
    Start {
        transfer_id: TransferId,
        direction: FileTransferDirection,
        filename: String,
        source_path: String,
        destination_path: String,
        total_size: u64,
        chunk_size: u32,
        sha256: [u8; 32],
    },
    Accept {
        transfer_id: TransferId,
        next_offset: u64,
    },
    Chunk {
        transfer_id: TransferId,
        offset: u64,
        checksum: [u8; 32],
        payload: Vec<u8>,
    },
    ChunkAck {
        transfer_id: TransferId,
        next_offset: u64,
    },
    Complete {
        transfer_id: TransferId,
        total_size: u64,
        sha256: [u8; 32],
    },
    Progress {
        transfer_id: TransferId,
        next_offset: u64,
    },
    Resume {
        transfer_id: TransferId,
        next_offset: u64,
    },
    Cancel {
        transfer_id: TransferId,
    },
    Error {
        request_id: Option<u64>,
        transfer_id: Option<TransferId>,
        code: FileTransferErrorCode,
        message: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Message {
    Control(ControlMessage),
    Video(EncodedVideoFrame),
    Input(InputEvent),
    Clipboard(ClipboardMessage),
    FileTransfer(FileTransferMessage),
}

impl Message {
    #[must_use]
    pub const fn channel(&self) -> Channel {
        match self {
            Self::Control(_) => Channel::Control,
            Self::Video(_) => Channel::Video,
            Self::Input(_) => Channel::Input,
            Self::Clipboard(_) => Channel::Clipboard,
            Self::FileTransfer(_) => Channel::FileTransfer,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MessageEnvelope {
    pub version: u16,
    pub session_id: SessionId,
    pub channel: Channel,
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub message: Message,
}

impl MessageEnvelope {
    #[must_use]
    pub fn new(session_id: SessionId, sequence: u64, timestamp_ms: u64, message: Message) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            session_id,
            channel: message.channel(),
            sequence,
            timestamp_ms,
            message,
        }
    }
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.version != PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion(self.version));
        }
        if self.channel != self.message.channel() {
            return Err(ProtocolError::ChannelMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ProtocolError {
    #[error("device ID must contain exactly nine ASCII digits")]
    InvalidDeviceId,
    #[error("display ID must be 1-128 characters without control characters")]
    InvalidDisplayId,
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("message type does not match envelope channel")]
    ChannelMismatch,
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ProtocolCodecError {
    #[error("protocol serialization failed: {0}")]
    Encode(String),
    #[error("protocol deserialization failed: {0}")]
    Decode(String),
    #[error("protocol message contains trailing bytes")]
    TrailingBytes,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_id_requires_nine_digits() {
        assert!(DeviceId::new("825371946").is_ok());
        assert_eq!(DeviceId::new("123"), Err(ProtocolError::InvalidDeviceId));
        assert_eq!(
            DeviceId::new("12345678x"),
            Err(ProtocolError::InvalidDeviceId)
        );
    }

    #[test]
    fn envelope_infers_and_validates_channel() {
        let envelope = MessageEnvelope::new(
            SessionId::new(),
            7,
            1_700_000_000_000,
            Message::Input(InputEvent::MouseMove {
                display_id: None,
                normalized_x: 10,
                normalized_y: 20,
            }),
        );
        assert_eq!(envelope.channel, Channel::Input);
        assert_eq!(envelope.validate(), Ok(()));
    }

    #[test]
    fn envelope_rejects_version_and_channel_mismatch() {
        let mut envelope = MessageEnvelope::new(
            SessionId::new(),
            0,
            0,
            Message::Control(ControlMessage::Ping { nonce: 42 }),
        );
        envelope.version = 99;
        assert_eq!(
            envelope.validate(),
            Err(ProtocolError::UnsupportedVersion(99))
        );
        envelope.version = PROTOCOL_VERSION;
        envelope.channel = Channel::Input;
        assert_eq!(envelope.validate(), Err(ProtocolError::ChannelMismatch));
    }

    #[test]
    fn binary_round_trip_preserves_envelope() {
        let original = MessageEnvelope::new(
            SessionId::new(),
            1,
            123,
            Message::Clipboard(ClipboardMessage::Text {
                origin: ClipboardOrigin::Controller,
                revision: 1,
                text: "hello".into(),
            }),
        );
        let bytes = encode_wire(&original).expect("encode test value");
        let decoded: MessageEnvelope = decode_wire(&bytes).expect("decode test value");
        assert_eq!(decoded, original);
    }

    #[test]
    fn relay_messages_use_the_central_wire_codec() {
        let original = RelayServerMessage::ProtocolError {
            code: RelayProtocolErrorCode::InvalidToken,
            message: "authentication failed".to_owned(),
        };
        let bytes = encode_wire(&original).expect("encode relay message");
        let decoded: RelayServerMessage = decode_wire(&bytes).expect("decode relay message");

        assert_eq!(decoded, original);
    }

    #[test]
    fn mouse_button_and_wheel_events_round_trip() {
        let events = [
            InputEvent::MouseButtonDown {
                button: MouseButton::Left,
            },
            InputEvent::MouseButtonUp {
                button: MouseButton::Right,
            },
            InputEvent::MouseWheel {
                axis: WheelAxis::Vertical,
                delta: -120,
            },
        ];

        for event in events {
            let bytes = encode_wire(&event).expect("encode input event");
            let decoded: InputEvent = decode_wire(&bytes).expect("decode input event");
            assert_eq!(decoded, event);
        }
    }

    #[test]
    fn display_id_rejects_empty_and_control_text() {
        assert!(DisplayId::new("0:1").is_ok());
        assert_eq!(DisplayId::new(""), Err(ProtocolError::InvalidDisplayId));
        assert_eq!(
            DisplayId::new("display\n1"),
            Err(ProtocolError::InvalidDisplayId)
        );
    }

    #[test]
    fn keyboard_and_modifier_events_round_trip() {
        let events = [
            InputEvent::KeyDown {
                key: KeyCode::ControlLeft,
            },
            InputEvent::KeyDown { key: KeyCode::KeyC },
            InputEvent::KeyUp { key: KeyCode::KeyC },
            InputEvent::KeyUp {
                key: KeyCode::ControlLeft,
            },
        ];

        for event in events {
            let bytes = encode_wire(&event).expect("encode keyboard event");
            let decoded: InputEvent = decode_wire(&bytes).expect("decode keyboard event");
            assert_eq!(decoded, event);
        }
    }

    #[test]
    fn utf8_clipboard_message_round_trips() {
        let original = ClipboardMessage::Text {
            origin: ClipboardOrigin::Agent,
            revision: 42,
            text: "中文と日本語".to_owned(),
        };
        let bytes = encode_wire(&original).expect("encode clipboard message");
        let decoded: ClipboardMessage = decode_wire(&bytes).expect("decode clipboard message");
        assert_eq!(decoded, original);
    }

    #[test]
    fn resumable_file_messages_round_trip() {
        let transfer_id = TransferId::new();
        let messages = [
            FileTransferMessage::Start {
                transfer_id,
                direction: FileTransferDirection::Upload,
                filename: "archive.bin".into(),
                source_path: String::new(),
                destination_path: "/Data/archive.bin".into(),
                total_size: 9_000_000,
                chunk_size: DEFAULT_FILE_CHUNK_SIZE,
                sha256: [7; 32],
            },
            FileTransferMessage::ChunkAck {
                transfer_id,
                next_offset: 4_512_366_592,
            },
            FileTransferMessage::Resume {
                transfer_id,
                next_offset: 4_512_366_592,
            },
        ];
        for message in messages {
            let encoded = encode_wire(&message).expect("encode file message");
            let decoded: FileTransferMessage = decode_wire(&encoded).expect("decode file message");
            assert_eq!(decoded, message);
        }
    }
}
