//! Windows DXGI Desktop Duplication implementation.
use crate::{CaptureError, Frame, MonitorId, MonitorInfo, PixelFormat, ScreenCapture};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0},
            Direct3D11::{
                D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
                D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
                D3D11_USAGE_STAGING, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
                ID3D11Texture2D,
            },
            Dxgi::{
                CreateDXGIFactory1, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_NOT_FOUND,
                DXGI_ERROR_WAIT_TIMEOUT, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO,
                DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR,
                DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR,
                DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME, IDXGIAdapter1, IDXGIFactory1,
                IDXGIOutput, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource,
            },
        },
    },
    core::{Error as WindowsError, Interface},
};

const ACQUIRE_TIMEOUT_MS: u32 = 100;
const BYTES_PER_PIXEL: usize = 4;

#[derive(Clone)]
struct OutputSelection {
    id: MonitorId,
    adapter: IDXGIAdapter1,
    output: IDXGIOutput,
    origin_x: i32,
    origin_y: i32,
}

#[derive(Clone, Default)]
struct PointerShape {
    visible: bool,
    x: i32,
    y: i32,
    info: DXGI_OUTDUPL_POINTER_SHAPE_INFO,
    bytes: Vec<u8>,
}

#[derive(Clone)]
struct CaptureResources {
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    origin_x: i32,
    origin_y: i32,
}

/// Captures a selected Windows display through DXGI Desktop Duplication.
pub struct DxgiCapture {
    selected: Option<OutputSelection>,
    resources: Option<CaptureResources>,
    pointer: PointerShape,
}

impl DxgiCapture {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            selected: None,
            resources: None,
            pointer: PointerShape {
                visible: false,
                x: 0,
                y: 0,
                info: DXGI_OUTDUPL_POINTER_SHAPE_INFO {
                    Type: 0,
                    Width: 0,
                    Height: 0,
                    Pitch: 0,
                    HotSpot: windows::Win32::Foundation::POINT { x: 0, y: 0 },
                },
                bytes: Vec::new(),
            },
        }
    }

    fn selections() -> Result<Vec<(OutputSelection, MonitorInfo)>, CaptureError> {
        // SAFETY: DXGI factory and COM interface methods are called with initialized output
        // values. The windows crate owns returned interface reference counts.
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(platform)?;
            let mut selections = Vec::new();
            let mut adapter_index = 0_u32;
            loop {
                let adapter = match factory.EnumAdapters1(adapter_index) {
                    Ok(adapter) => adapter,
                    Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                    Err(error) => return Err(platform(error)),
                };
                let mut output_index = 0_u32;
                loop {
                    let output = match adapter.EnumOutputs(output_index) {
                        Ok(output) => output,
                        Err(error) if error.code() == DXGI_ERROR_NOT_FOUND => break,
                        Err(error) => return Err(platform(error)),
                    };
                    let desc = output.GetDesc().map_err(platform)?;
                    if desc.AttachedToDesktop.as_bool() {
                        let id = MonitorId(format!("{adapter_index}:{output_index}"));
                        let width = coordinate_extent(
                            desc.DesktopCoordinates.left,
                            desc.DesktopCoordinates.right,
                        )?;
                        let height = coordinate_extent(
                            desc.DesktopCoordinates.top,
                            desc.DesktopCoordinates.bottom,
                        )?;
                        let name_length = desc
                            .DeviceName
                            .iter()
                            .position(|value| *value == 0)
                            .unwrap_or(desc.DeviceName.len());
                        let name = String::from_utf16_lossy(&desc.DeviceName[..name_length]);
                        let info = MonitorInfo {
                            id: id.clone(),
                            name,
                            width,
                            height,
                            origin_x: desc.DesktopCoordinates.left,
                            origin_y: desc.DesktopCoordinates.top,
                            is_primary: desc.DesktopCoordinates.left == 0
                                && desc.DesktopCoordinates.top == 0,
                        };
                        selections.push((
                            OutputSelection {
                                id,
                                adapter: adapter.clone(),
                                output,
                                origin_x: desc.DesktopCoordinates.left,
                                origin_y: desc.DesktopCoordinates.top,
                            },
                            info,
                        ));
                    }
                    output_index += 1;
                }
                adapter_index += 1;
            }
            Ok(selections)
        }
    }

    fn create_resources(selection: &OutputSelection) -> Result<CaptureResources, CaptureError> {
        // SAFETY: D3D11 receives a valid enumerated adapter and initialized out-parameters.
        // Resource descriptors use the dimensions and format reported by DXGI duplication.
        unsafe {
            let mut device = None;
            let mut context = None;
            D3D11CreateDevice(
                &selection.adapter,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&raw mut device),
                None,
                Some(&raw mut context),
            )
            .map_err(platform)?;
            let device =
                device.ok_or_else(|| CaptureError::Other("D3D11 returned no device".into()))?;
            let context =
                context.ok_or_else(|| CaptureError::Other("D3D11 returned no context".into()))?;
            let output: IDXGIOutput1 = selection.output.cast().map_err(platform)?;
            let duplication = output.DuplicateOutput(&device).map_err(platform)?;
            let duplication_desc = duplication.GetDesc();
            let width = duplication_desc.ModeDesc.Width;
            let height = duplication_desc.ModeDesc.Height;
            let texture_desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: duplication_desc.ModeDesc.Format,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            device
                .CreateTexture2D(&raw const texture_desc, None, Some(&raw mut staging))
                .map_err(platform)?;
            Ok(CaptureResources {
                _device: device,
                context,
                duplication,
                staging: staging.ok_or_else(|| {
                    CaptureError::Other("D3D11 returned no staging texture".into())
                })?,
                width,
                height,
                origin_x: selection.origin_x,
                origin_y: selection.origin_y,
            })
        }
    }

    fn recreate(&mut self) -> Result<(), CaptureError> {
        let selection = self.selected.as_ref().ok_or(CaptureError::NotRunning)?;
        self.resources = Some(Self::create_resources(selection)?);
        Ok(())
    }

    fn capture_once(&mut self) -> Result<Frame, CaptureError> {
        let resources = self.resources.clone().ok_or(CaptureError::NotRunning)?;
        // SAFETY: A frame is released by FrameLease on every path after successful
        // acquisition. The mapped pointer remains valid only until Unmap and is copied first.
        unsafe {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut desktop_resource: Option<IDXGIResource> = None;
            resources
                .duplication
                .AcquireNextFrame(
                    ACQUIRE_TIMEOUT_MS,
                    &raw mut frame_info,
                    &raw mut desktop_resource,
                )
                .map_err(classify_acquire_error)?;
            let _lease = FrameLease(resources.duplication.clone());
            let desktop_resource = desktop_resource
                .ok_or_else(|| CaptureError::Other("DXGI returned no desktop resource".into()))?;
            let desktop_texture: ID3D11Texture2D = desktop_resource.cast().map_err(platform)?;
            resources
                .context
                .CopyResource(&resources.staging, &desktop_texture);

            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            resources
                .context
                .Map(
                    &resources.staging,
                    0,
                    D3D11_MAP_READ,
                    0,
                    Some(&raw mut mapped),
                )
                .map_err(platform)?;
            let compact_stride = usize::try_from(resources.width)
                .map_err(|_| CaptureError::Other("frame width is out of range".into()))?
                .checked_mul(BYTES_PER_PIXEL)
                .ok_or_else(|| CaptureError::Other("frame stride overflow".into()))?;
            let row_pitch = usize::try_from(mapped.RowPitch)
                .map_err(|_| CaptureError::Other("row pitch is out of range".into()))?;
            let height = usize::try_from(resources.height)
                .map_err(|_| CaptureError::Other("frame height is out of range".into()))?;
            let mapped_length = row_pitch
                .checked_mul(height)
                .ok_or_else(|| CaptureError::Other("mapped texture length overflow".into()))?;
            let source = std::slice::from_raw_parts(mapped.pData.cast::<u8>(), mapped_length);
            let mut data = vec![0_u8; compact_stride * height];
            for row in 0..height {
                let source_start = row * row_pitch;
                let destination_start = row * compact_stride;
                data[destination_start..destination_start + compact_stride]
                    .copy_from_slice(&source[source_start..source_start + compact_stride]);
            }
            resources.context.Unmap(&resources.staging, 0);

            self.update_pointer(&frame_info)?;
            composite_pointer(&mut data, resources.width, resources.height, &self.pointer);
            Ok(Frame {
                width: resources.width,
                height: resources.height,
                stride: u32::try_from(compact_stride)
                    .map_err(|_| CaptureError::Other("frame stride is out of range".into()))?,
                pixel_format: PixelFormat::Bgra8,
                timestamp_ms: timestamp_ms()?,
                data,
            })
        }
    }

    fn update_pointer(&mut self, frame_info: &DXGI_OUTDUPL_FRAME_INFO) -> Result<(), CaptureError> {
        let resources = self.resources.as_ref().ok_or(CaptureError::NotRunning)?;
        if frame_info.LastMouseUpdateTime != 0 {
            self.pointer.visible = frame_info.PointerPosition.Visible.as_bool();
            self.pointer.x = frame_info.PointerPosition.Position.x - resources.origin_x;
            self.pointer.y = frame_info.PointerPosition.Position.y - resources.origin_y;
        }
        if frame_info.PointerShapeBufferSize == 0 {
            return Ok(());
        }

        self.pointer
            .bytes
            .resize(frame_info.PointerShapeBufferSize as usize, 0);
        let mut required = 0_u32;
        // SAFETY: The shape buffer is sized using PointerShapeBufferSize from the acquired
        // frame. DXGI writes at most that capacity and initializes both output values.
        unsafe {
            resources
                .duplication
                .GetFramePointerShape(
                    frame_info.PointerShapeBufferSize,
                    self.pointer.bytes.as_mut_ptr().cast(),
                    &raw mut required,
                    &raw mut self.pointer.info,
                )
                .map_err(platform)?;
        }
        self.pointer.bytes.truncate(required as usize);
        Ok(())
    }
}

impl Default for DxgiCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenCapture for DxgiCapture {
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError> {
        Ok(Self::selections()?
            .into_iter()
            .map(|(_, info)| info)
            .collect())
    }

    fn start(&mut self, monitor: &MonitorId) -> Result<(), CaptureError> {
        let selection = Self::selections()?
            .into_iter()
            .find(|(selection, _)| selection.id == *monitor)
            .map(|(selection, _)| selection)
            .ok_or(CaptureError::MonitorNotFound)?;
        self.resources = Some(Self::create_resources(&selection)?);
        self.selected = Some(selection);
        Ok(())
    }

    fn next_frame(&mut self) -> Result<Frame, CaptureError> {
        match self.capture_once() {
            Err(CaptureError::DisplayModeChanged) => {
                self.resources = None;
                self.recreate()?;
                self.capture_once()
            }
            result => result,
        }
    }

    fn stop(&mut self) -> Result<(), CaptureError> {
        self.resources = None;
        self.selected = None;
        self.pointer = PointerShape::default();
        Ok(())
    }
}

struct FrameLease(IDXGIOutputDuplication);

impl Drop for FrameLease {
    fn drop(&mut self) {
        // SAFETY: FrameLease is constructed only after AcquireNextFrame succeeds and is
        // dropped exactly once.
        let _result = unsafe { self.0.ReleaseFrame() };
    }
}

fn composite_pointer(data: &mut [u8], width: u32, height: u32, pointer: &PointerShape) {
    if !pointer.visible || pointer.bytes.is_empty() {
        return;
    }
    match pointer.info.Type {
        value if value == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_COLOR.0 as u32 => {
            composite_color(data, width, height, pointer, false);
        }
        value if value == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MASKED_COLOR.0 as u32 => {
            composite_color(data, width, height, pointer, true);
        }
        value if value == DXGI_OUTDUPL_POINTER_SHAPE_TYPE_MONOCHROME.0 as u32 => {
            composite_monochrome(data, width, height, pointer);
        }
        _ => {}
    }
}

fn composite_color(
    data: &mut [u8],
    frame_width: u32,
    frame_height: u32,
    pointer: &PointerShape,
    masked: bool,
) {
    let pitch = pointer.info.Pitch as usize;
    for y in 0..pointer.info.Height {
        for x in 0..pointer.info.Width {
            let source = y as usize * pitch + x as usize * BYTES_PER_PIXEL;
            if source + 3 >= pointer.bytes.len() {
                continue;
            }
            let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
                continue;
            };
            let destination_x = pointer.x + x;
            let destination_y = pointer.y + y;
            let Some(destination) =
                pixel_offset(destination_x, destination_y, frame_width, frame_height)
            else {
                continue;
            };
            let alpha = pointer.bytes[source + 3];
            for channel in 0..3 {
                let foreground = pointer.bytes[source + channel];
                if masked && alpha != 0 {
                    data[destination + channel] ^= foreground;
                } else {
                    data[destination + channel] = blend(
                        foreground,
                        data[destination + channel],
                        if masked { 255 } else { alpha },
                    );
                }
            }
            data[destination + 3] = 255;
        }
    }
}

fn composite_monochrome(data: &mut [u8], width: u32, height: u32, pointer: &PointerShape) {
    let visible_height = pointer.info.Height / 2;
    let pitch = pointer.info.Pitch as usize;
    let xor_base = pitch * visible_height as usize;
    for y in 0..visible_height {
        for x in 0..pointer.info.Width {
            let byte = y as usize * pitch + x as usize / 8;
            if xor_base + byte >= pointer.bytes.len() {
                continue;
            }
            let bit = 7 - (x as usize % 8);
            let and_mask = (pointer.bytes[byte] >> bit) & 1;
            let xor_mask = (pointer.bytes[xor_base + byte] >> bit) & 1;
            let (Ok(x), Ok(y)) = (i32::try_from(x), i32::try_from(y)) else {
                continue;
            };
            let Some(destination) = pixel_offset(pointer.x + x, pointer.y + y, width, height)
            else {
                continue;
            };
            for channel in 0..3 {
                let preserved = if and_mask == 1 {
                    data[destination + channel]
                } else {
                    0
                };
                data[destination + channel] = preserved ^ if xor_mask == 1 { 255 } else { 0 };
            }
            data[destination + 3] = 255;
        }
    }
}

fn pixel_offset(x: i32, y: i32, width: u32, height: u32) -> Option<usize> {
    let x = u32::try_from(x).ok()?;
    let y = u32::try_from(y).ok()?;
    if x >= width || y >= height {
        return None;
    }
    Some((y as usize * width as usize + x as usize) * BYTES_PER_PIXEL)
}

fn blend(foreground: u8, background: u8, alpha: u8) -> u8 {
    let alpha = u16::from(alpha);
    let value = u16::from(foreground) * alpha + u16::from(background) * (255 - alpha);
    u8::try_from(value / 255).unwrap_or(u8::MAX)
}

fn coordinate_extent(start: i32, end: i32) -> Result<u32, CaptureError> {
    u32::try_from(end - start)
        .map_err(|_| CaptureError::Other("monitor coordinates are invalid".into()))
}

fn classify_acquire_error(error: WindowsError) -> CaptureError {
    if error.code() == DXGI_ERROR_WAIT_TIMEOUT {
        CaptureError::Timeout
    } else if error.code() == DXGI_ERROR_ACCESS_LOST {
        CaptureError::DisplayModeChanged
    } else {
        platform(error)
    }
}

#[allow(clippy::needless_pass_by_value)]
fn platform(error: WindowsError) -> CaptureError {
    CaptureError::Other(error.to_string())
}

fn timestamp_ms() -> Result<u64, CaptureError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CaptureError::Other("system clock is before the Unix epoch".into()))?;
    u64::try_from(duration.as_millis())
        .map_err(|_| CaptureError::Other("timestamp is out of range".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_blend_has_expected_endpoints() {
        assert_eq!(blend(200, 10, 0), 10);
        assert_eq!(blend(200, 10, 255), 200);
    }

    #[test]
    fn pointer_clipping_rejects_pixels_outside_frame() {
        assert_eq!(pixel_offset(-1, 0, 10, 10), None);
        assert_eq!(pixel_offset(10, 0, 10, 10), None);
        assert_eq!(pixel_offset(2, 3, 10, 10), Some(128));
    }
}
