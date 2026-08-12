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
    let mut controller = MockPeer::connect(Role::Controller).await?;
    let ping = MessageEnvelope::new(
        session_id,
        0,
        now_ms()?,
        Message::Control(ControlMessage::Ping { nonce: 42 }),
    );
    controller.send_payload(encode_wire(&ping)?).await?;
    info!(event = "mock_controller_sent", nonce = 42);

    let response: MessageEnvelope = decode_wire(&controller.receive_payload().await?)?;
    response.validate()?;
    match response.message {
        Message::Control(ControlMessage::Pong { nonce: 42 }) => {
            info!(event = "mock_controller_received", nonce = 42);
        }
        other => anyhow::bail!("unexpected Agent response: {other:?}"),
    }
    controller.close().await
}
