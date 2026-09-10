//! Negotiated remote-desktop video codecs, adaptive quality, and decoding.

use image::{DynamicImage, ImageEncoder, ImageFormat, RgbaImage, imageops::FilterType};
use openh264::{
    OpenH264API,
    decoder::Decoder,
    encoder::{
        BitRate, Encoder as OpenH264Encoder, EncoderConfig as OpenH264Config, FrameRate, FrameType,
        IntraFramePeriod, RateControlMode, UsageType,
    },
    formats::{RgbaSliceU8, YUVBuffer, YUVSource},
};
use remotex_capture::{Frame, PixelFormat};
use remotex_protocol::{DiagnosticValue, EncodedVideoFrame, VideoCodec, VideoFeedback};
use std::time::Instant;
use thiserror::Error;

pub const MAX_VIDEO_PIXELS: u64 = 3840 * 2160;
pub const MAX_ENCODED_VIDEO_SIZE: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncoderConfig {
    pub maximum_width: u32,
    pub maximum_height: u32,
    pub jpeg_quality: u8,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            maximum_width: 1280,
            maximum_height: 720,
            jpeg_quality: 72,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EncodeContext {
    pub frame_id: u64,
    pub frames_per_second: u32,
    pub bitrate_bps: u32,
    pub capture_latency_ms: u32,
}

pub trait VideoEncoder: Send {
    fn codec(&self) -> VideoCodec;
    fn encode_frame(
        &mut self,
        frame: &Frame,
        context: EncodeContext,
    ) -> Result<EncodedVideoFrame, VideoError>;
}

pub trait VideoDecoder: Send {
    fn decode_frame(&mut self, frame: &EncodedVideoFrame) -> Result<DecodedFrame, VideoError>;
}

pub struct SoftwareEncoder {
    config: EncoderConfig,
}

impl SoftwareEncoder {
    pub fn new(config: EncoderConfig) -> Result<Self, VideoError> {
        validate_encoder_config(config)?;
        Ok(Self { config })
    }

    pub fn encode(&self, frame: &Frame) -> Result<EncodedVideoFrame, VideoError> {
        let mut encoder = Self {
            config: self.config,
        };
        encoder.encode_frame(
            frame,
            EncodeContext {
                frame_id: 0,
                frames_per_second: 12,
                bitrate_bps: 0,
                capture_latency_ms: 0,
            },
        )
    }
}

impl VideoEncoder for SoftwareEncoder {
    fn codec(&self) -> VideoCodec {
        VideoCodec::Jpeg
    }

    fn encode_frame(
        &mut self,
        frame: &Frame,
        context: EncodeContext,
    ) -> Result<EncodedVideoFrame, VideoError> {
        let started = Instant::now();
        let (width, height, rgba) = prepare_rgba(
            frame,
            self.config.maximum_width,
            self.config.maximum_height,
            false,
        )?;
        let rgb = DynamicImage::ImageRgba8(
            RgbaImage::from_raw(width, height, rgba).ok_or(VideoError::InvalidBufferLength)?,
        )
        .into_rgb8();
        let mut payload = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut payload, self.config.jpeg_quality)
            .write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)
            .map_err(VideoError::Image)?;
        encoded_frame(
            context,
            frame.timestamp_ms,
            width,
            height,
            VideoCodec::Jpeg,
            true,
            payload,
            elapsed_ms(started),
        )
    }
}

pub struct H264Encoder {
    encoder: OpenH264Encoder,
    profile: QualityProfile,
}

impl H264Encoder {
    pub fn new(profile: QualityProfile) -> Result<Self, VideoError> {
        #[allow(clippy::cast_precision_loss)]
        let maximum_frame_rate = FrameRate::from_hz(profile.frames_per_second as f32);
        let config = OpenH264Config::new()
            .bitrate(BitRate::from_bps(profile.bitrate_bps))
            .max_frame_rate(maximum_frame_rate)
            .rate_control_mode(RateControlMode::Bitrate)
            .usage_type(UsageType::ScreenContentRealTime)
            .skip_frames(true)
            .intra_frame_period(IntraFramePeriod::from_num_frames(
                profile.frames_per_second.saturating_mul(2),
            ));
        let encoder = OpenH264Encoder::with_api_config(OpenH264API::from_source(), config)
            .map_err(|error| VideoError::H264(error.to_string()))?;
        Ok(Self { encoder, profile })
    }
}

impl VideoEncoder for H264Encoder {
    fn codec(&self) -> VideoCodec {
        VideoCodec::H264
    }

    fn encode_frame(
        &mut self,
        frame: &Frame,
        context: EncodeContext,
    ) -> Result<EncodedVideoFrame, VideoError> {
        let started = Instant::now();
        let (width, height, rgba) = prepare_rgba(
            frame,
            self.profile.maximum_width,
            self.profile.maximum_height,
            true,
        )?;
        let dimensions = (
            usize::try_from(width).map_err(|_| VideoError::InvalidDimensions)?,
            usize::try_from(height).map_err(|_| VideoError::InvalidDimensions)?,
        );
        let source = RgbaSliceU8::new(&rgba, dimensions);
        let yuv = YUVBuffer::from_rgb_source(source);
        let bitstream = self
            .encoder
            .encode(&yuv)
            .map_err(|error| VideoError::H264(error.to_string()))?;
        let key_frame = matches!(bitstream.frame_type(), FrameType::IDR | FrameType::I);
        encoded_frame(
            context,
            frame.timestamp_ms,
            width,
            height,
            VideoCodec::H264,
            key_frame,
            bitstream.to_vec(),
            elapsed_ms(started),
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum QualityLevel {
    Poor,
    Medium,
    Good,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum VideoPerformanceProfile {
    #[default]
    Auto,
    Quality,
    Balanced,
    LowBandwidth,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QualityProfile {
    pub maximum_width: u32,
    pub maximum_height: u32,
    pub frames_per_second: u32,
    pub bitrate_bps: u32,
}

impl QualityLevel {
    #[must_use]
    pub const fn profile(self, maximum_fps: u32) -> QualityProfile {
        let (maximum_width, maximum_height, frames_per_second, bitrate_bps) = match self {
            Self::Poor => (1280, 720, 10, 800_000),
            Self::Medium => (1600, 900, 20, 2_000_000),
            Self::Good => (1920, 1080, 30, 4_000_000),
        };
        QualityProfile {
            maximum_width,
            maximum_height,
            frames_per_second: if maximum_fps < frames_per_second {
                maximum_fps
            } else {
                frames_per_second
            },
            bitrate_bps,
        }
    }
}

pub struct AdaptiveQualityController {
    level: QualityLevel,
    maximum_fps: u32,
    poor_streak: u8,
    good_streak: u8,
    minimum_level: QualityLevel,
    maximum_level: QualityLevel,
}

impl AdaptiveQualityController {
    pub fn new(maximum_fps: u32) -> Result<Self, VideoError> {
        Self::for_profile(maximum_fps, VideoPerformanceProfile::Auto)
    }

    pub fn for_profile(
        maximum_fps: u32,
        profile: VideoPerformanceProfile,
    ) -> Result<Self, VideoError> {
        if !(1..=60).contains(&maximum_fps) {
            return Err(VideoError::InvalidFrameRate);
        }
        let (level, minimum_level, maximum_level) = match profile {
            VideoPerformanceProfile::Auto => {
                (QualityLevel::Good, QualityLevel::Poor, QualityLevel::Good)
            }
            VideoPerformanceProfile::Quality => {
                (QualityLevel::Good, QualityLevel::Medium, QualityLevel::Good)
            }
            VideoPerformanceProfile::Balanced => (
                QualityLevel::Medium,
                QualityLevel::Poor,
                QualityLevel::Medium,
            ),
            VideoPerformanceProfile::LowBandwidth => {
                (QualityLevel::Poor, QualityLevel::Poor, QualityLevel::Poor)
            }
        };
        Ok(Self {
            level,
            maximum_fps,
            poor_streak: 0,
            good_streak: 0,
            minimum_level,
            maximum_level,
        })
    }

    #[must_use]
    pub const fn level(&self) -> QualityLevel {
        self.level
    }

    #[must_use]
    pub const fn profile(&self) -> QualityProfile {
        self.level.profile(self.maximum_fps)
    }

    pub fn observe(&mut self, feedback: VideoFeedback) -> bool {
        let rtt = diagnostic_value(feedback.rtt_ms);
        let loss = diagnostic_value(feedback.packet_loss_per_mille);
        let queue = diagnostic_value(feedback.send_queue_percent);
        let decode = diagnostic_value(feedback.decoder_latency_ms);
        let render = diagnostic_value(feedback.render_latency_ms);
        let dropped = diagnostic_value(feedback.dropped_frames_per_mille);
        let poor = loss.is_some_and(|value| value >= 50)
            || rtt.is_some_and(|value| value >= 180)
            || queue.is_some_and(|value| value >= 75)
            || decode.is_some_and(|value| value >= 80)
            || render.is_some_and(|value| value >= 50)
            || dropped.is_some_and(|value| value >= 100);
        let available = [
            rtt.is_some(),
            loss.is_some(),
            queue.is_some(),
            decode.is_some(),
        ]
        .into_iter()
        .filter(|value| *value)
        .count();
        let good = available >= 2
            && loss.is_none_or(|value| value <= 10)
            && rtt.is_none_or(|value| value <= 80)
            && queue.is_none_or(|value| value <= 30)
            && decode.is_none_or(|value| value <= 30)
            && render.is_none_or(|value| value <= 20)
            && dropped.is_none_or(|value| value <= 20);
        if poor {
            self.poor_streak = self.poor_streak.saturating_add(1);
            self.good_streak = 0;
        } else if good {
            self.good_streak = self.good_streak.saturating_add(1);
            self.poor_streak = 0;
        } else {
            self.poor_streak = 0;
            self.good_streak = 0;
        }
        let previous = self.level;
        if self.poor_streak >= 3 {
            self.level = match self.level {
                QualityLevel::Good => QualityLevel::Medium,
                QualityLevel::Medium | QualityLevel::Poor => QualityLevel::Poor,
            }
            .max(self.minimum_level);
            self.poor_streak = 0;
        } else if self.good_streak >= 6 {
            self.level = match self.level {
                QualityLevel::Poor => QualityLevel::Medium,
                QualityLevel::Medium | QualityLevel::Good => QualityLevel::Good,
            }
            .min(self.maximum_level);
            self.good_streak = 0;
        }
        self.level != previous
    }
}

fn diagnostic_value<T: Copy>(value: DiagnosticValue<T>) -> Option<T> {
    match value {
        DiagnosticValue::Available { value, .. } => Some(value),
        DiagnosticValue::Unavailable => None,
    }
}

pub struct SessionVideoEncoder {
    codec: VideoCodec,
    adaptive: AdaptiveQualityController,
    jpeg: SoftwareEncoder,
    h264: Option<H264Encoder>,
    frame_id: u64,
    last_frame_timestamp_ms: Option<u64>,
}

impl SessionVideoEncoder {
    pub fn new(maximum_fps: u32) -> Result<Self, VideoError> {
        Self::with_profile(maximum_fps, VideoPerformanceProfile::Auto)
    }

    pub fn with_profile(
        maximum_fps: u32,
        performance_profile: VideoPerformanceProfile,
    ) -> Result<Self, VideoError> {
        let adaptive = AdaptiveQualityController::for_profile(maximum_fps, performance_profile)?;
        let profile = adaptive.profile();
        Ok(Self {
            codec: VideoCodec::Jpeg,
            adaptive,
            jpeg: jpeg_for_profile(profile)?,
            h264: None,
            frame_id: 0,
            last_frame_timestamp_ms: None,
        })
    }

    #[must_use]
    pub const fn codec(&self) -> VideoCodec {
        self.codec
    }

    #[must_use]
    pub const fn profile(&self) -> QualityProfile {
        self.adaptive.profile()
    }

    pub fn negotiate(&mut self, offered: &[VideoCodec]) -> Result<VideoCodec, VideoError> {
        let selected = negotiate_codec(offered, &[VideoCodec::H264, VideoCodec::Jpeg])
            .ok_or(VideoError::NoCommonCodec)?;
        self.codec = selected;
        self.rebuild_encoders()?;
        Ok(selected)
    }

    pub fn apply_feedback(&mut self, feedback: VideoFeedback) -> Result<bool, VideoError> {
        if self.adaptive.observe(feedback) {
            self.rebuild_encoders()?;
            return Ok(true);
        }
        Ok(false)
    }

    #[must_use]
    pub fn frame_due(&self, timestamp_ms: u64) -> bool {
        let interval = 1000 / u64::from(self.profile().frames_per_second.max(1));
        self.last_frame_timestamp_ms
            .is_none_or(|previous| timestamp_ms.saturating_sub(previous) >= interval)
    }

    pub fn encode(
        &mut self,
        frame: &Frame,
        capture_latency_ms: u32,
    ) -> Result<EncodedVideoFrame, VideoError> {
        let profile = self.profile();
        let context = EncodeContext {
            frame_id: self.frame_id,
            frames_per_second: profile.frames_per_second,
            bitrate_bps: profile.bitrate_bps,
            capture_latency_ms,
        };
        let encoded = match self.codec {
            VideoCodec::H264 => {
                if self.h264.is_none() {
                    self.h264 = Some(H264Encoder::new(profile)?);
                }
                let primary = self
                    .h264
                    .as_mut()
                    .ok_or(VideoError::EncoderUnavailable)?
                    .encode_frame(frame, context);
                encode_with_fallback(primary, || self.jpeg.encode_frame(frame, context))
                    .inspect_err(|_| self.codec = VideoCodec::Jpeg)?
            }
            VideoCodec::Jpeg | VideoCodec::WebP => self.jpeg.encode_frame(frame, context)?,
        };
        if encoded.codec == VideoCodec::Jpeg && self.codec == VideoCodec::H264 {
            self.codec = VideoCodec::Jpeg;
        }
        self.frame_id = self
            .frame_id
            .checked_add(1)
            .ok_or(VideoError::FrameIdExhausted)?;
        self.last_frame_timestamp_ms = Some(frame.timestamp_ms);
        Ok(encoded)
    }

    fn rebuild_encoders(&mut self) -> Result<(), VideoError> {
        let profile = self.profile();
        self.jpeg = jpeg_for_profile(profile)?;
        self.h264 = if self.codec == VideoCodec::H264 {
            Some(H264Encoder::new(profile)?)
        } else {
            None
        };
        self.last_frame_timestamp_ms = None;
        Ok(())
    }
}

pub struct StreamDecoder {
    h264: Decoder,
}

impl StreamDecoder {
    pub fn new() -> Result<Self, VideoError> {
        Ok(Self {
            h264: Decoder::new().map_err(|error| VideoError::H264(error.to_string()))?,
        })
    }
}

impl VideoDecoder for StreamDecoder {
    fn decode_frame(&mut self, frame: &EncodedVideoFrame) -> Result<DecodedFrame, VideoError> {
        validate_encoded_frame(frame)?;
        let started = Instant::now();
        let (width, height, rgba) = match frame.codec {
            VideoCodec::H264 => {
                let decoded = self
                    .h264
                    .decode(&frame.payload)
                    .map_err(|error| VideoError::H264(error.to_string()))?
                    .ok_or(VideoError::DecoderNotReady)?;
                let (width, height) = decoded.dimensions();
                let mut rgba = vec![0; decoded.rgba8_len()];
                decoded.write_rgba8(&mut rgba);
                (
                    u32::try_from(width).map_err(|_| VideoError::InvalidDimensions)?,
                    u32::try_from(height).map_err(|_| VideoError::InvalidDimensions)?,
                    rgba,
                )
            }
            VideoCodec::Jpeg | VideoCodec::WebP => {
                let format = match frame.codec {
                    VideoCodec::Jpeg => ImageFormat::Jpeg,
                    VideoCodec::WebP => ImageFormat::WebP,
                    VideoCodec::H264 => unreachable!(),
                };
                let decoded = image::load_from_memory_with_format(&frame.payload, format)
                    .map_err(VideoError::Image)?
                    .into_rgba8();
                (decoded.width(), decoded.height(), decoded.into_raw())
            }
        };
        if width != frame.width || height != frame.height {
            return Err(VideoError::DecodedDimensionsMismatch);
        }
        Ok(DecodedFrame {
            width,
            height,
            rgba,
            decode_latency_ms: elapsed_ms(started),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub decode_latency_ms: u32,
}

pub fn decode(frame: &EncodedVideoFrame) -> Result<DecodedFrame, VideoError> {
    StreamDecoder::new()?.decode_frame(frame)
}

#[must_use]
pub fn negotiate_codec(controller: &[VideoCodec], agent: &[VideoCodec]) -> Option<VideoCodec> {
    [VideoCodec::H264, VideoCodec::WebP, VideoCodec::Jpeg]
        .into_iter()
        .find(|codec| controller.contains(codec) && agent.contains(codec))
}

fn encode_with_fallback<T>(
    primary: Result<T, VideoError>,
    fallback: impl FnOnce() -> Result<T, VideoError>,
) -> Result<T, VideoError> {
    primary.or_else(|_| fallback())
}

fn jpeg_for_profile(profile: QualityProfile) -> Result<SoftwareEncoder, VideoError> {
    SoftwareEncoder::new(EncoderConfig {
        maximum_width: profile.maximum_width,
        maximum_height: profile.maximum_height,
        jpeg_quality: match profile.bitrate_bps {
            0..=1_000_000 => 55,
            1_000_001..=2_500_000 => 68,
            _ => 78,
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn encoded_frame(
    context: EncodeContext,
    source_timestamp_ms: u64,
    width: u32,
    height: u32,
    codec: VideoCodec,
    key_frame: bool,
    payload: Vec<u8>,
    encode_latency_ms: u32,
) -> Result<EncodedVideoFrame, VideoError> {
    if payload.is_empty() || payload.len() > MAX_ENCODED_VIDEO_SIZE {
        return Err(VideoError::EncodedPayloadLimit);
    }
    Ok(EncodedVideoFrame {
        frame_id: context.frame_id,
        width,
        height,
        frames_per_second: context.frames_per_second,
        bitrate_bps: context.bitrate_bps,
        source_timestamp_ms,
        capture_latency_ms: context.capture_latency_ms,
        encode_latency_ms,
        codec,
        key_frame,
        payload,
    })
}

fn prepare_rgba(
    frame: &Frame,
    maximum_width: u32,
    maximum_height: u32,
    require_even: bool,
) -> Result<(u32, u32, Vec<u8>), VideoError> {
    validate_capture_frame(frame)?;
    let mut rgba = frame.data.clone();
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    let image = RgbaImage::from_raw(frame.width, frame.height, rgba)
        .ok_or(VideoError::InvalidBufferLength)?;
    let (mut width, mut height) =
        fitted_dimensions(frame.width, frame.height, maximum_width, maximum_height)?;
    if require_even {
        width = (width & !1).max(2);
        height = (height & !1).max(2);
    }
    let image = if width == frame.width && height == frame.height {
        image
    } else {
        image::imageops::resize(&image, width, height, FilterType::Triangle)
    };
    Ok((width, height, image.into_raw()))
}

fn validate_capture_frame(frame: &Frame) -> Result<(), VideoError> {
    if frame.pixel_format != PixelFormat::Bgra8 {
        return Err(VideoError::UnsupportedPixelFormat);
    }
    let expected_stride = frame
        .width
        .checked_mul(4)
        .ok_or(VideoError::InvalidDimensions)?;
    if frame.stride != expected_stride {
        return Err(VideoError::NonCompactStride);
    }
    let expected_length = usize::try_from(frame.stride)
        .ok()
        .and_then(|stride| stride.checked_mul(frame.height as usize))
        .ok_or(VideoError::InvalidDimensions)?;
    if frame.data.len() != expected_length {
        return Err(VideoError::InvalidBufferLength);
    }
    validate_dimensions(frame.width, frame.height)
}

fn validate_encoded_frame(frame: &EncodedVideoFrame) -> Result<(), VideoError> {
    validate_dimensions(frame.width, frame.height)?;
    if frame.payload.is_empty() || frame.payload.len() > MAX_ENCODED_VIDEO_SIZE {
        return Err(VideoError::EncodedPayloadLimit);
    }
    if frame.frames_per_second == 0 || frame.frames_per_second > 60 {
        return Err(VideoError::InvalidFrameRate);
    }
    Ok(())
}

fn validate_dimensions(width: u32, height: u32) -> Result<(), VideoError> {
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_VIDEO_PIXELS {
        return Err(VideoError::InvalidDimensions);
    }
    Ok(())
}

fn validate_encoder_config(config: EncoderConfig) -> Result<(), VideoError> {
    validate_dimensions(config.maximum_width, config.maximum_height)?;
    if config.jpeg_quality == 0 || config.jpeg_quality > 100 {
        return Err(VideoError::InvalidQuality);
    }
    Ok(())
}

fn fitted_dimensions(
    width: u32,
    height: u32,
    maximum_width: u32,
    maximum_height: u32,
) -> Result<(u32, u32), VideoError> {
    validate_dimensions(width, height)?;
    validate_dimensions(maximum_width, maximum_height)?;
    if width <= maximum_width && height <= maximum_height {
        return Ok((width, height));
    }
    let width_limited = u64::from(width) * u64::from(maximum_height)
        >= u64::from(height) * u64::from(maximum_width);
    let (fitted_width, fitted_height) = if width_limited {
        let height = (u64::from(height) * u64::from(maximum_width) + u64::from(width) / 2)
            / u64::from(width);
        (
            maximum_width,
            u32::try_from(height).map_err(|_| VideoError::InvalidDimensions)?,
        )
    } else {
        let width = (u64::from(width) * u64::from(maximum_height) + u64::from(height) / 2)
            / u64::from(height);
        (
            u32::try_from(width).map_err(|_| VideoError::InvalidDimensions)?,
            maximum_height,
        )
    };
    Ok((fitted_width.max(1), fitted_height.max(1)))
}

fn elapsed_ms(started: Instant) -> u32 {
    started.elapsed().as_millis().try_into().unwrap_or(u32::MAX)
}

#[derive(Debug, Error)]
pub enum VideoError {
    #[error("frame dimensions must be non-zero, bounded, and representable")]
    InvalidDimensions,
    #[error("frame rate must be between 1 and 60")]
    InvalidFrameRate,
    #[error("JPEG quality must be between 1 and 100")]
    InvalidQuality,
    #[error("only compact BGRA8 capture frames are supported")]
    UnsupportedPixelFormat,
    #[error("capture frame stride must be width multiplied by four")]
    NonCompactStride,
    #[error("capture frame buffer length does not match its dimensions")]
    InvalidBufferLength,
    #[error("decoded dimensions do not match the protocol metadata")]
    DecodedDimensionsMismatch,
    #[error("encoded video payload is empty or exceeds its limit")]
    EncodedPayloadLimit,
    #[error("no mutually supported video codec")]
    NoCommonCodec,
    #[error("video encoder is unavailable")]
    EncoderUnavailable,
    #[error("video decoder needs more stream data")]
    DecoderNotReady,
    #[error("video frame ID space is exhausted")]
    FrameIdExhausted,
    #[error("image codec failed: {0}")]
    Image(#[source] image::ImageError),
    #[error("H.264 codec failed: {0}")]
    H264(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use remotex_protocol::MetricProvenance;

    fn solid_frame(width: u32, height: u32) -> Frame {
        let mut data = vec![0_u8; width as usize * height as usize * 4];
        for pixel in data.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[10, 80, 200, 255]);
        }
        Frame {
            width,
            height,
            stride: width * 4,
            pixel_format: PixelFormat::Bgra8,
            timestamp_ms: 42,
            data,
        }
    }

    #[test]
    fn scales_full_hd_to_720p_and_round_trips_jpeg() {
        let codec = SoftwareEncoder::new(EncoderConfig::default()).expect("config");
        let encoded = codec.encode(&solid_frame(1920, 1080)).expect("encode");
        assert_eq!((encoded.width, encoded.height), (1280, 720));
        assert_eq!(encoded.codec, VideoCodec::Jpeg);
        let decoded = decode(&encoded).expect("decode");
        assert_eq!((decoded.width, decoded.height), (1280, 720));
        assert_eq!(decoded.rgba.len(), 1280 * 720 * 4);
    }

    #[test]
    fn h264_stream_round_trips_and_supports_resolution_restart() {
        let profile = QualityLevel::Poor.profile(30);
        let mut h264_encoder = H264Encoder::new(profile).expect("encoder");
        let mut stream_decoder = StreamDecoder::new().expect("decoder");
        for (frame_id, dimensions) in [(0, (640, 480)), (1, (320, 240))] {
            let encoded_frame = h264_encoder
                .encode_frame(
                    &solid_frame(dimensions.0, dimensions.1),
                    EncodeContext {
                        frame_id,
                        frames_per_second: 10,
                        bitrate_bps: 800_000,
                        capture_latency_ms: 1,
                    },
                )
                .expect("encode h264");
            let decoded_frame = stream_decoder
                .decode_frame(&encoded_frame)
                .expect("decode h264");
            assert_eq!((decoded_frame.width, decoded_frame.height), dimensions);
        }
    }

    #[test]
    fn negotiation_prefers_h264_and_rejects_no_common_codec() {
        assert_eq!(
            negotiate_codec(
                &[VideoCodec::Jpeg, VideoCodec::H264],
                &[VideoCodec::H264, VideoCodec::Jpeg]
            ),
            Some(VideoCodec::H264)
        );
        assert_eq!(
            negotiate_codec(&[VideoCodec::WebP], &[VideoCodec::H264]),
            None
        );
    }

    #[test]
    fn adaptive_quality_uses_hysteresis() {
        let mut adaptive = AdaptiveQualityController::new(30).expect("adaptive");
        let poor = VideoFeedback {
            rtt_ms: measured(250),
            packet_loss_per_mille: measured(80),
            send_queue_percent: measured(90),
            decoder_latency_ms: measured(100),
            render_latency_ms: measured(30),
            dropped_frames_per_mille: measured(150),
        };
        assert!(!adaptive.observe(poor));
        assert!(!adaptive.observe(poor));
        assert!(adaptive.observe(poor));
        assert_eq!(adaptive.level(), QualityLevel::Medium);
        let good = VideoFeedback {
            rtt_ms: measured(20),
            packet_loss_per_mille: measured(0),
            send_queue_percent: measured(5),
            decoder_latency_ms: measured(5),
            render_latency_ms: measured(5),
            dropped_frames_per_mille: measured(0),
        };
        for _ in 0..5 {
            assert!(!adaptive.observe(good));
        }
        assert!(adaptive.observe(good));
        assert_eq!(adaptive.level(), QualityLevel::Good);
    }

    #[test]
    fn user_profiles_bound_the_adaptive_ladder() {
        let quality = AdaptiveQualityController::for_profile(30, VideoPerformanceProfile::Quality)
            .expect("quality profile");
        assert_eq!(quality.level(), QualityLevel::Good);

        let balanced =
            AdaptiveQualityController::for_profile(20, VideoPerformanceProfile::Balanced)
                .expect("balanced profile");
        assert_eq!(balanced.level(), QualityLevel::Medium);
        assert_eq!(balanced.profile().frames_per_second, 20);

        let mut low =
            AdaptiveQualityController::for_profile(10, VideoPerformanceProfile::LowBandwidth)
                .expect("low bandwidth profile");
        assert_eq!(low.level(), QualityLevel::Poor);
        let good = VideoFeedback {
            rtt_ms: measured(10),
            packet_loss_per_mille: measured(0),
            send_queue_percent: measured(0),
            decoder_latency_ms: measured(1),
            render_latency_ms: measured(1),
            dropped_frames_per_mille: measured(0),
        };
        for _ in 0..12 {
            assert!(!low.observe(good));
        }
        assert_eq!(low.level(), QualityLevel::Poor);
    }

    fn measured<T>(value: T) -> DiagnosticValue<T> {
        DiagnosticValue::Available {
            value,
            provenance: MetricProvenance::Measured,
        }
    }

    #[test]
    fn encoder_error_calls_fallback() {
        let value = encode_with_fallback::<u8>(Err(VideoError::EncoderUnavailable), || Ok(7))
            .expect("fallback");
        assert_eq!(value, 7);
    }

    #[test]
    fn malformed_video_metadata_is_rejected_before_decode() {
        let frame = EncodedVideoFrame {
            frame_id: 0,
            width: u32::MAX,
            height: u32::MAX,
            frames_per_second: 30,
            bitrate_bps: 1,
            source_timestamp_ms: 0,
            capture_latency_ms: 0,
            encode_latency_ms: 0,
            codec: VideoCodec::H264,
            key_frame: true,
            payload: vec![1],
        };
        assert!(matches!(decode(&frame), Err(VideoError::InvalidDimensions)));
    }
}
