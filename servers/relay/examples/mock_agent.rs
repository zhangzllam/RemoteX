mod support;

use remotex_protocol::{
    ControlMessage, Message, MessageEnvelope, Role, SessionId, decode_wire, encode_wire,
};
use support::{MockPeer, initialize_tracing, now_ms};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    initialize_tracing()?;
    let session_id: SessionId = std::env::var("REMOTEX_SESSION_ID")?.parse()?;
    let mut agent = MockPeer::connect(Role::Agent).await?;
    let request: MessageEnvelope = decode_wire(&agent.receive_payload().await?)?;
    request.validate()?;
    let Message::Control(ControlMessage::Ping { nonce }) = request.message else {
        anyhow::bail!("mock Agent expected ControlMessage::Ping");
    };
    info!(event = "mock_agent_received", nonce);
    let response = MessageEnvelope::new(
        session_id,
        request.sequence,
        now_ms()?,
        Message::Control(ControlMessage::Pong { nonce }),
    );
    agent.send_payload(encode_wire(&response)?).await?;
    info!(event = "mock_agent_sent", nonce);
    agent.close().await
}
