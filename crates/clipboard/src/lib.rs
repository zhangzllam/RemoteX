//! Permissioned, bounded plain-text clipboard synchronization with loop prevention.

use remotex_protocol::{ClipboardMessage, ClipboardOrigin, MAX_CLIPBOARD_TEXT_SIZE};
use thiserror::Error;

#[derive(Debug, Error, Eq, PartialEq)]
pub enum ClipboardError {
    #[error("clipboard permission denied")]
    PermissionDenied,
    #[error("clipboard text exceeds the {MAX_CLIPBOARD_TEXT_SIZE}-byte UTF-8 limit")]
    TextTooLarge,
    #[error("clipboard revision must be nonzero and newer than the last applied revision")]
    InvalidRevision,
    #[error("clipboard message has the local origin")]
    InvalidOrigin,
    #[error("clipboard revision space exhausted")]
    RevisionExhausted,
    #[error("clipboard text is not valid UTF-16")]
    InvalidUtf16,
    #[error("clipboard platform operation failed: {0}")]
    Platform(String),
}

pub trait ClipboardBackend: Send {
    fn read_text(&mut self) -> Result<String, ClipboardError>;
    fn write_text(&mut self, text: &str) -> Result<(), ClipboardError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyResult {
    Applied,
    IgnoredStale,
    IgnoredConflict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ClipboardStamp {
    origin: ClipboardOrigin,
    revision: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClipboardState {
    origin: ClipboardOrigin,
    next_revision: u64,
    last_remote_revision: u64,
    last_local_text: Option<String>,
    current_stamp: Option<ClipboardStamp>,
}

impl ClipboardState {
    #[must_use]
    pub const fn origin(&self) -> ClipboardOrigin {
        self.origin
    }

    #[must_use]
    pub const fn last_remote_revision(&self) -> u64 {
        self.last_remote_revision
    }
}

pub struct PermissionedClipboard<B: ClipboardBackend> {
    backend: B,
    state: ClipboardState,
    clipboard: bool,
}

impl<B: ClipboardBackend> PermissionedClipboard<B> {
    #[must_use]
    pub fn new(backend: B, origin: ClipboardOrigin, clipboard: bool) -> Self {
        Self {
            backend,
            state: ClipboardState {
                origin,
                next_revision: 1,
                last_remote_revision: 0,
                last_local_text: None,
                current_stamp: None,
            },
            clipboard,
        }
    }

    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.clipboard
    }

    #[must_use]
    pub const fn state(&self) -> &ClipboardState {
        &self.state
    }

    pub fn poll(&mut self) -> Result<Option<ClipboardMessage>, ClipboardError> {
        self.ensure_permission()?;
        let text = self.backend.read_text()?;
        validate_text(&text)?;
        if self.state.last_local_text.as_ref() == Some(&text) {
            return Ok(None);
        }
        let revision = self.state.next_revision;
        self.state.next_revision = revision
            .checked_add(1)
            .ok_or(ClipboardError::RevisionExhausted)?;
        self.state.last_local_text = Some(text.clone());
        self.state.current_stamp = Some(ClipboardStamp {
            origin: self.state.origin,
            revision,
        });
        Ok(Some(ClipboardMessage::Text {
            origin: self.state.origin,
            revision,
            text,
        }))
    }

    pub fn apply(&mut self, message: ClipboardMessage) -> Result<ApplyResult, ClipboardError> {
        self.ensure_permission()?;
        let ClipboardMessage::Text {
            origin,
            revision,
            text,
        } = message;
        if origin == self.state.origin {
            return Err(ClipboardError::InvalidOrigin);
        }
        validate_text(&text)?;
        if revision == 0 {
            return Err(ClipboardError::InvalidRevision);
        }
        if revision <= self.state.last_remote_revision {
            return Ok(ApplyResult::IgnoredStale);
        }
        let next_revision = revision
            .checked_add(1)
            .ok_or(ClipboardError::RevisionExhausted)?;
        let remote_stamp = ClipboardStamp { origin, revision };
        if self
            .state
            .current_stamp
            .is_some_and(|current| !stamp_wins(remote_stamp, current))
        {
            self.state.last_remote_revision = revision;
            self.state.next_revision = self.state.next_revision.max(next_revision);
            return Ok(ApplyResult::IgnoredConflict);
        }
        self.backend.write_text(&text)?;
        self.state.last_remote_revision = revision;
        self.state.next_revision = self.state.next_revision.max(next_revision);
        self.state.last_local_text = Some(text);
        self.state.current_stamp = Some(remote_stamp);
        Ok(ApplyResult::Applied)
    }

    fn ensure_permission(&self) -> Result<(), ClipboardError> {
        if self.clipboard {
            Ok(())
        } else {
            Err(ClipboardError::PermissionDenied)
        }
    }
}

fn stamp_wins(candidate: ClipboardStamp, current: ClipboardStamp) -> bool {
    candidate.revision > current.revision
        || (candidate.revision == current.revision
            && candidate.origin == ClipboardOrigin::Controller
            && current.origin == ClipboardOrigin::Agent)
}

fn validate_text(text: &str) -> Result<(), ClipboardError> {
    if text.len() > MAX_CLIPBOARD_TEXT_SIZE {
        Err(ClipboardError::TextTooLarge)
    } else {
        Ok(())
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows;

#[cfg(windows)]
pub use windows::WindowsClipboardBackend;

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct MemoryClipboard {
        text: String,
        writes: usize,
    }

    impl ClipboardBackend for MemoryClipboard {
        fn read_text(&mut self) -> Result<String, ClipboardError> {
            Ok(self.text.clone())
        }

        fn write_text(&mut self, text: &str) -> Result<(), ClipboardError> {
            self.text = text.to_owned();
            self.writes += 1;
            Ok(())
        }
    }

    #[test]
    fn utf8_chinese_and_japanese_sync_bidirectionally() {
        let mut controller = PermissionedClipboard::new(
            MemoryClipboard {
                text: "中文と日本語".to_owned(),
                writes: 0,
            },
            ClipboardOrigin::Controller,
            true,
        );
        let mut agent =
            PermissionedClipboard::new(MemoryClipboard::default(), ClipboardOrigin::Agent, true);

        let message = controller.poll().expect("poll controller").expect("change");
        assert_eq!(agent.apply(message), Ok(ApplyResult::Applied));
        assert_eq!(agent.backend.text, "中文と日本語");
        assert_eq!(agent.poll(), Ok(None));
    }

    #[test]
    fn empty_clipboard_is_a_valid_change() {
        let mut clipboard = PermissionedClipboard::new(
            MemoryClipboard::default(),
            ClipboardOrigin::Controller,
            true,
        );
        assert_eq!(
            clipboard.poll(),
            Ok(Some(ClipboardMessage::Text {
                origin: ClipboardOrigin::Controller,
                revision: 1,
                text: String::new(),
            }))
        );
    }

    #[test]
    fn oversized_local_and_remote_text_is_rejected() {
        let oversized = "x".repeat(MAX_CLIPBOARD_TEXT_SIZE + 1);
        let mut local = PermissionedClipboard::new(
            MemoryClipboard {
                text: oversized.clone(),
                writes: 0,
            },
            ClipboardOrigin::Controller,
            true,
        );
        assert_eq!(local.poll(), Err(ClipboardError::TextTooLarge));
        assert_eq!(
            local.apply(ClipboardMessage::Text {
                origin: ClipboardOrigin::Agent,
                revision: 1,
                text: oversized,
            }),
            Err(ClipboardError::TextTooLarge)
        );
    }

    #[test]
    fn applying_remote_text_does_not_echo_it_back() {
        let mut clipboard =
            PermissionedClipboard::new(MemoryClipboard::default(), ClipboardOrigin::Agent, true);
        let message = ClipboardMessage::Text {
            origin: ClipboardOrigin::Controller,
            revision: 7,
            text: "no echo".to_owned(),
        };
        assert_eq!(clipboard.apply(message.clone()), Ok(ApplyResult::Applied));
        assert_eq!(clipboard.poll(), Ok(None));
        assert_eq!(clipboard.apply(message), Ok(ApplyResult::IgnoredStale));
        assert_eq!(clipboard.backend.writes, 1);
    }

    #[test]
    fn simultaneous_initial_values_converge_without_echo() {
        let mut controller = PermissionedClipboard::new(
            MemoryClipboard {
                text: "controller".to_owned(),
                writes: 0,
            },
            ClipboardOrigin::Controller,
            true,
        );
        let mut agent = PermissionedClipboard::new(
            MemoryClipboard {
                text: "agent".to_owned(),
                writes: 0,
            },
            ClipboardOrigin::Agent,
            true,
        );
        let controller_message = controller.poll().expect("poll").expect("change");
        let agent_message = agent.poll().expect("poll").expect("change");

        assert_eq!(agent.apply(controller_message), Ok(ApplyResult::Applied));
        assert_eq!(
            controller.apply(agent_message),
            Ok(ApplyResult::IgnoredConflict)
        );
        assert_eq!(controller.backend.text, "controller");
        assert_eq!(agent.backend.text, "controller");
        assert_eq!(controller.poll(), Ok(None));
        assert_eq!(agent.poll(), Ok(None));
    }

    #[test]
    fn disabled_permission_blocks_read_and_write() {
        let mut clipboard =
            PermissionedClipboard::new(MemoryClipboard::default(), ClipboardOrigin::Agent, false);
        assert_eq!(clipboard.poll(), Err(ClipboardError::PermissionDenied));
        assert_eq!(
            clipboard.apply(ClipboardMessage::Text {
                origin: ClipboardOrigin::Controller,
                revision: 1,
                text: "blocked".to_owned(),
            }),
            Err(ClipboardError::PermissionDenied)
        );
        assert_eq!(clipboard.backend.writes, 0);
    }

    #[test]
    fn zero_revision_and_local_origin_are_rejected() {
        let mut clipboard =
            PermissionedClipboard::new(MemoryClipboard::default(), ClipboardOrigin::Agent, true);
        assert_eq!(
            clipboard.apply(ClipboardMessage::Text {
                origin: ClipboardOrigin::Controller,
                revision: 0,
                text: "invalid".to_owned(),
            }),
            Err(ClipboardError::InvalidRevision)
        );
        assert_eq!(
            clipboard.apply(ClipboardMessage::Text {
                origin: ClipboardOrigin::Agent,
                revision: 1,
                text: "wrong origin".to_owned(),
            }),
            Err(ClipboardError::InvalidOrigin)
        );
    }
}
