//! Pure resumable-transfer state validation. M0 performs no filesystem I/O.

use remotex_protocol::TransferId;
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferMetadata {
    pub transfer_id: TransferId,
    pub filename: String,
    pub destination_path: String,
    pub total_size: u64,
    pub chunk_size: u32,
    pub sha256: [u8; 32],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferStatus {
    Active,
    Paused,
    Completed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TransferTracker {
    metadata: TransferMetadata,
    next_offset: u64,
    status: TransferStatus,
}

impl TransferTracker {
    pub fn new(metadata: TransferMetadata) -> Result<Self, TransferError> {
        if metadata.chunk_size == 0 {
            return Err(TransferError::InvalidChunkSize);
        }
        Ok(Self {
            metadata,
            next_offset: 0,
            status: TransferStatus::Active,
        })
    }
    #[must_use]
    pub const fn metadata(&self) -> &TransferMetadata {
        &self.metadata
    }
    #[must_use]
    pub const fn next_offset(&self) -> u64 {
        self.next_offset
    }
    #[must_use]
    pub const fn status(&self) -> TransferStatus {
        self.status
    }

    pub fn accept_chunk(&mut self, offset: u64, length: usize) -> Result<(), TransferError> {
        if self.status != TransferStatus::Active {
            return Err(TransferError::NotActive);
        }
        if offset != self.next_offset {
            return Err(TransferError::UnexpectedOffset {
                expected: self.next_offset,
                actual: offset,
            });
        }
        let length = u64::try_from(length).map_err(|_| TransferError::ChunkTooLarge)?;
        if length == 0 || length > u64::from(self.metadata.chunk_size) {
            return Err(TransferError::ChunkTooLarge);
        }
        let next = offset
            .checked_add(length)
            .ok_or(TransferError::ExceedsDeclaredSize)?;
        if next > self.metadata.total_size {
            return Err(TransferError::ExceedsDeclaredSize);
        }
        self.next_offset = next;
        Ok(())
    }

    pub fn pause(&mut self) -> Result<(), TransferError> {
        if self.status != TransferStatus::Active {
            return Err(TransferError::NotActive);
        }
        self.status = TransferStatus::Paused;
        Ok(())
    }

    pub fn resume(&mut self, next_offset: u64) -> Result<(), TransferError> {
        if self.status != TransferStatus::Paused {
            return Err(TransferError::NotPaused);
        }
        if next_offset != self.next_offset {
            return Err(TransferError::UnexpectedOffset {
                expected: self.next_offset,
                actual: next_offset,
            });
        }
        self.status = TransferStatus::Active;
        Ok(())
    }

    pub fn complete(&mut self) -> Result<(), TransferError> {
        if self.status != TransferStatus::Active {
            return Err(TransferError::NotActive);
        }
        if self.next_offset != self.metadata.total_size {
            return Err(TransferError::Incomplete {
                expected: self.metadata.total_size,
                actual: self.next_offset,
            });
        }
        self.status = TransferStatus::Completed;
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), TransferError> {
        if matches!(
            self.status,
            TransferStatus::Completed | TransferStatus::Cancelled
        ) {
            return Err(TransferError::Terminal);
        }
        self.status = TransferStatus::Cancelled;
        Ok(())
    }
}

#[derive(Debug, Error, Eq, PartialEq)]
pub enum TransferError {
    #[error("chunk size must be non-zero")]
    InvalidChunkSize,
    #[error("transfer is not active")]
    NotActive,
    #[error("transfer is not paused")]
    NotPaused,
    #[error("expected offset {expected}, received {actual}")]
    UnexpectedOffset { expected: u64, actual: u64 },
    #[error("chunk is empty or exceeds the negotiated chunk size")]
    ChunkTooLarge,
    #[error("chunk exceeds the declared file size")]
    ExceedsDeclaredSize,
    #[error("transfer is incomplete: expected {expected} bytes, received {actual}")]
    Incomplete { expected: u64, actual: u64 },
    #[error("transfer is already in a terminal state")]
    Terminal,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> TransferMetadata {
        TransferMetadata {
            transfer_id: TransferId::new(),
            filename: "a.bin".into(),
            destination_path: "inbox".into(),
            total_size: 10,
            chunk_size: 4,
            sha256: [0; 32],
        }
    }

    #[test]
    fn tracks_chunks_and_completion() {
        let mut tracker = TransferTracker::new(metadata()).expect("valid metadata");
        tracker.accept_chunk(0, 4).expect("first chunk");
        tracker.accept_chunk(4, 4).expect("second chunk");
        tracker.accept_chunk(8, 2).expect("final chunk");
        tracker.complete().expect("complete");
        assert_eq!(tracker.status(), TransferStatus::Completed);
    }

    #[test]
    fn rejects_wrong_offset_and_oversized_chunk() {
        let mut tracker = TransferTracker::new(metadata()).expect("valid metadata");
        assert_eq!(
            tracker.accept_chunk(1, 4),
            Err(TransferError::UnexpectedOffset {
                expected: 0,
                actual: 1
            })
        );
        assert_eq!(
            tracker.accept_chunk(0, 5),
            Err(TransferError::ChunkTooLarge)
        );
    }

    #[test]
    fn pause_and_resume_require_matching_offset() {
        let mut tracker = TransferTracker::new(metadata()).expect("valid metadata");
        tracker.accept_chunk(0, 4).expect("chunk");
        tracker.pause().expect("pause");
        assert_eq!(
            tracker.resume(0),
            Err(TransferError::UnexpectedOffset {
                expected: 4,
                actual: 0
            })
        );
        tracker.resume(4).expect("resume");
        assert_eq!(tracker.status(), TransferStatus::Active);
    }
}
