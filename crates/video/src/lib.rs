//! M3 software image encoding, decoding, and 720p scaling.

use image::{DynamicImage, ImageEncoder, ImageFormat, RgbaImage, imageops::FilterType};
use remotex_capture::{Frame, PixelFormat};
use remotex_protocol::{EncodedVideoFrame, VideoCodec};
use thiserror::Error;

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

pub struct SoftwareEncoder {
    config: EncoderConfig,
}

impl SoftwareEncoder {
    pub fn new(config: EncoderConfig) -> Result<Self, VideoError> {
        if config.maximum_width == 0 || config.maximum_height == 0 {
            return Err(VideoError::InvalidDimensions);
        }
        if config.jpeg_quality == 0 || config.jpeg_quality > 100 {
            return Err(VideoError::InvalidQuality);
        }
        Ok(Self { config })
    }

    pub fn encode(&self, frame: &Frame) -> Result<EncodedVideoFrame, VideoError> {
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

        let mut rgba = frame.data.clone();
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.swap(0, 2);
            pixel[3] = 255;
        }
        let image = RgbaImage::from_raw(frame.width, frame.height, rgba)
            .ok_or(VideoError::InvalidBufferLength)?;
        let (width, height) = fitted_dimensions(
            frame.width,
            frame.height,
            self.config.maximum_width,
            self.config.maximum_height,
        )?;
        let image = if width == frame.width && height == frame.height {
            image
        } else {
            image::imageops::resize(&image, width, height, FilterType::Triangle)
        };
        let rgb = DynamicImage::ImageRgba8(image).into_rgb8();
        let mut payload = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut payload, self.config.jpeg_quality)
            .write_image(&rgb, width, height, image::ExtendedColorType::Rgb8)
            .map_err(VideoError::Image)?;
        Ok(EncodedVideoFrame {
            width,
            height,
            source_timestamp_ms: frame.timestamp_ms,
            codec: VideoCodec::Jpeg,
            key_frame: true,
            payload,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub fn decode(frame: &EncodedVideoFrame) -> Result<DecodedFrame, VideoError> {
    let format = match frame.codec {
        VideoCodec::Jpeg => ImageFormat::Jpeg,
        VideoCodec::WebP => ImageFormat::WebP,
    };
    let decoded = image::load_from_memory_with_format(&frame.payload, format)
        .map_err(VideoError::Image)?
        .into_rgba8();
    if decoded.width() != frame.width || decoded.height() != frame.height {
        return Err(VideoError::DecodedDimensionsMismatch);
    }
    Ok(DecodedFrame {
        width: decoded.width(),
        height: decoded.height(),
        rgba: decoded.into_raw(),
    })
}

fn fitted_dimensions(
    width: u32,
    height: u32,
    maximum_width: u32,
    maximum_height: u32,
) -> Result<(u32, u32), VideoError> {
    if width == 0 || height == 0 {
        return Err(VideoError::InvalidDimensions);
    }
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

#[derive(Debug, Error)]
pub enum VideoError {
    #[error("frame dimensions must be non-zero and representable")]
    InvalidDimensions,
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
    #[error("image codec failed: {0}")]
    Image(#[source] image::ImageError),
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!encoded.payload.is_empty());
        let decoded = decode(&encoded).expect("decode");
        assert_eq!((decoded.width, decoded.height), (1280, 720));
        assert_eq!(decoded.rgba.len(), 1280 * 720 * 4);
    }

    #[test]
    fn preserves_smaller_frames() {
        let codec = SoftwareEncoder::new(EncoderConfig::default()).expect("config");
        let encoded = codec.encode(&solid_frame(640, 480)).expect("encode");
        assert_eq!((encoded.width, encoded.height), (640, 480));
    }
}
