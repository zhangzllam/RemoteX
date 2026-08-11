//! Transport-neutral asynchronous byte-frame contracts.

use async_trait::async_trait;
use bytes::Bytes;
use thiserror::Error;

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
}
