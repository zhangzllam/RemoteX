//! Versioned, platform-neutral `RemoteX` protocol domain types.

use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};
use thiserror::Error;
use uuid::Uuid;

pub const PROTOCOL_VERSION: u16 = 1;
pub const DEFAULT_FILE_CHUNK_SIZE: u32 = 4 * 1024 * 1024;

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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TransferId(Uuid);

impl TransferId {
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}
impl Default for TransferId {
    fn default() -> Self {
        Self::new()
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

/// First application frame sent by a peer after opening its QUIC stream.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelayHandshake {
    pub version: u16,
    pub session_id: SessionId,
    pub role: Role,
    pub token: SessionToken,
}

impl RelayHandshake {
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct SessionPermissions {
    pub view_desktop: bool,
    pub control_input: bool,
    pub clipboard: bool,
    pub file_upload: bool,
    pub file_download: bool,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    MouseMove {
        normalized_x: u16,
        normalized_y: u16,
    },
    MouseButton {
        button: MouseButton,
        state: ButtonState,
    },
    MouseWheel {
        axis: WheelAxis,
        delta: i32,
    },
    Key {
        usage: u32,
        state: ButtonState,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ClipboardMessage {
    Text { content: String },
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FileTransferMessage {
    Start {
        transfer_id: TransferId,
        filename: String,
        destination_path: String,
        total_size: u64,
        chunk_size: u32,
        sha256: [u8; 32],
    },
    Chunk {
        transfer_id: TransferId,
        offset: u64,
        checksum: [u8; 32],
        payload: Vec<u8>,
    },
    End {
        transfer_id: TransferId,
        total_size: u64,
        sha256: [u8; 32],
    },
    Progress {
        transfer_id: TransferId,
        next_offset: u64,
    },
    Pause {
        transfer_id: TransferId,
    },
    Resume {
        transfer_id: TransferId,
        next_offset: u64,
    },
    Cancel {
        transfer_id: TransferId,
    },
    Error {
        transfer_id: TransferId,
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
    #[error("unsupported protocol version {0}")]
    UnsupportedVersion(u16),
    #[error("message type does not match envelope channel")]
    ChannelMismatch,
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
                content: "hello".into(),
            }),
        );
        let config = bincode::config::standard();
        let bytes = bincode::serde::encode_to_vec(&original, config).expect("encode test value");
        let (decoded, consumed): (MessageEnvelope, usize) =
            bincode::serde::decode_from_slice(&bytes, config).expect("decode test value");
        assert_eq!(consumed, bytes.len());
        assert_eq!(decoded, original);
    }
}
