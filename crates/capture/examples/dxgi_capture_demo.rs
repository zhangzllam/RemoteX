#[cfg(windows)]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use image::{ImageBuffer, Rgba};
    use remotex_capture::{DxgiCapture, PixelFormat, ScreenCapture};

    let mut capture = DxgiCapture::new();
    let monitors = capture.monitors()?;
    for (index, monitor) in monitors.iter().enumerate() {
        println!(
            "[{index}] {} ({}x{}, primary={})",
            monitor.name, monitor.width, monitor.height, monitor.is_primary
        );
    }
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.is_primary)
        .or_else(|| monitors.first())
        .ok_or("no attached monitor found")?;
    capture.start(&monitor.id)?;

    let mut captured = 0_u32;
    let final_frame = loop {
        match capture.next_frame() {
            Ok(frame) => {
                captured += 1;
                println!("captured frame {captured}/100");
                if captured == 100 {
                    break frame;
                }
            }
            Err(remotex_capture::CaptureError::Timeout) => {}
            Err(error) => return Err(error.into()),
        }
    };
    capture.stop()?;
    if final_frame.pixel_format != PixelFormat::Bgra8 {
        return Err("unexpected capture pixel format".into());
    }
    let mut rgba = final_frame.data;
    for pixel in rgba.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    let image: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_raw(final_frame.width, final_frame.height, rgba)
            .ok_or("captured frame dimensions do not match its buffer")?;
    image.save("remotex-dxgi-frame-100.png")?;
    println!("saved remotex-dxgi-frame-100.png");
    Ok(())
}

#[cfg(not(windows))]
fn main() {
    eprintln!("the DXGI capture demo is available only on Windows");
}
