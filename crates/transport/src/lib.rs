//! Transport-neutral asynchronous byte-frame contracts.

use async_trait::async_trait;
use bytes::Bytes;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Default maximum application frame accepted by transport readers (8 MiB).
pub const DEFAULT_MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub server_name: String,
}

#[derive(Debug, Error)]
pub enum TransportError {
    #[error("connection closed")]
    Closed,
    #[error("frame size {actual} exceeds limit {maximum}")]
    FrameTooLarge { actual: usize, maximum: usize },
    #[error("transport failed: {0}")]
    Other(String),
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("QUIC read failed: {0}")]
    QuicRead(#[from] quinn::ReadError),
    #[error("QUIC write failed: {0}")]
    QuicWrite(#[from] quinn::WriteError),
    #[error("QUIC stream is already closed: {0}")]
    QuicClosed(#[from] quinn::ClosedStream),
}

/// Reads one big-endian length-prefixed frame with a pre-allocation size check.
pub async fn read_frame<R>(reader: &mut R, maximum: usize) -> Result<Bytes, TransportError>
where
    R: AsyncRead + Unpin,
{
    let length = reader.read_u32().await? as usize;
    if length > maximum {
        return Err(TransportError::FrameTooLarge {
            actual: length,
            maximum,
        });
    }
    let mut payload = vec![0_u8; length];
    reader.read_exact(&mut payload).await?;
    Ok(Bytes::from(payload))
}

/// Writes one big-endian length-prefixed frame after enforcing the size limit.
pub async fn write_frame<W>(
    writer: &mut W,
    frame: &[u8],
    maximum: usize,
) -> Result<(), TransportError>
where
    W: AsyncWrite + Unpin,
{
    if frame.len() > maximum {
        return Err(TransportError::FrameTooLarge {
            actual: frame.len(),
            maximum,
        });
    }
    let length = u32::try_from(frame.len()).map_err(|_| TransportError::FrameTooLarge {
        actual: frame.len(),
        maximum,
    })?;
    writer.write_u32(length).await?;
    writer.write_all(frame).await?;
    writer.flush().await?;
    Ok(())
}

/// A single framed bidirectional stream carried by a QUIC connection.
pub struct QuicFrameConnection {
    send: quinn::SendStream,
    receive: quinn::RecvStream,
    maximum: usize,
}

impl QuicFrameConnection {
    #[must_use]
    pub const fn new(send: quinn::SendStream, receive: quinn::RecvStream, maximum: usize) -> Self {
        Self {
            send,
            receive,
            maximum,
        }
    }
}

#[async_trait]
impl Connection for QuicFrameConnection {
    async fn send(&mut self, frame: Bytes) -> Result<(), TransportError> {
        write_frame(&mut self.send, &frame, self.maximum).await
    }

    async fn receive(&mut self) -> Result<Bytes, TransportError> {
        read_frame(&mut self.receive, self.maximum).await
    }

    async fn close(&mut self) -> Result<(), TransportError> {
        self.send.finish()?;
        Ok(())
    }
}

#[async_trait]
pub trait Connection: Send {
    async fn send(&mut self, frame: Bytes) -> Result<(), TransportError>;
    async fn receive(&mut self) -> Result<Bytes, TransportError>;
    async fn close(&mut self) -> Result<(), TransportError>;
}

#[async_trait]
pub trait Connector: Send + Sync {
    type Connection: Connection;
    async fn connect(&self, endpoint: &Endpoint) -> Result<Self::Connection, TransportError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct MemoryConnection {
        frames: VecDeque<Bytes>,
        closed: bool,
    }

    #[async_trait]
    impl Connection for MemoryConnection {
        async fn send(&mut self, frame: Bytes) -> Result<(), TransportError> {
            if self.closed {
                return Err(TransportError::Closed);
            }
            self.frames.push_back(frame);
            Ok(())
        }
        async fn receive(&mut self) -> Result<Bytes, TransportError> {
            self.frames.pop_front().ok_or(TransportError::Closed)
        }
        async fn close(&mut self) -> Result<(), TransportError> {
            self.closed = true;
            Ok(())
        }
    }

    #[tokio::test]
    async fn contract_can_be_implemented_without_networking() {
        let mut connection = MemoryConnection {
            frames: VecDeque::new(),
            closed: false,
        };
        connection
            .send(Bytes::from_static(b"frame"))
            .await
            .expect("send");
        assert_eq!(
            connection.receive().await.expect("receive"),
            Bytes::from_static(b"frame")
        );
        connection.close().await.expect("close");
        assert!(matches!(
            connection.send(Bytes::new()).await,
            Err(TransportError::Closed)
        ));
    }

    #[tokio::test]
    async fn framing_round_trips_and_rejects_oversized_frames() {
        let (mut left, mut right) = tokio::io::duplex(64);
        let writer = tokio::spawn(async move { write_frame(&mut left, b"hello", 16).await });
        assert_eq!(
            read_frame(&mut right, 16).await.expect("read"),
            Bytes::from_static(b"hello")
        );
        writer.await.expect("join").expect("write");

        let (mut left, mut right) = tokio::io::duplex(64);
        left.write_u32(17).await.expect("length");
        assert!(matches!(
            read_frame(&mut right, 16).await,
            Err(TransportError::FrameTooLarge {
                actual: 17,
                maximum: 16
            })
        ));
    }
}
