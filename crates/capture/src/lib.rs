//! Platform-neutral screen-capture model. M0 contains no capture implementation.

use thiserror::Error;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MonitorId(pub String);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MonitorInfo {
    pub id: MonitorId,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PixelFormat {
    Bgra8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixel_format: PixelFormat,
    pub timestamp_ms: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("monitor not found")]
    MonitorNotFound,
    #[error("capture is not running")]
    NotRunning,
    #[error("display mode changed")]
    DisplayModeChanged,
    #[error("capture failed: {0}")]
    Other(String),
}

pub trait ScreenCapture: Send {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError>;
    fn start(&mut self, monitor: &MonitorId) -> Result<(), CaptureError>;
    fn next_frame(&mut self) -> Result<Frame, CaptureError>;
    fn stop(&mut self) -> Result<(), CaptureError>;
}
