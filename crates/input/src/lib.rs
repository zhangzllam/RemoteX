//! Platform input execution contract. M0 performs no input injection.

use remotex_protocol::InputEvent;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum InputError {
    #[error("input permission denied")]
    PermissionDenied,
    #[error("unsupported input event")]
    Unsupported,
    #[error("input execution failed: {0}")]
    Other(String),
}

pub trait InputController: Send {
    fn apply(&mut self, event: InputEvent) -> Result<(), InputError>;
}
