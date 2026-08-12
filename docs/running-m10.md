# Running M10 video optimization

M10 negotiates H.264 after the encrypted session is ready. The Windows Agent
starts with the compatible JPEG encoder, switches to the bundled OpenH264
software encoder when the Controller advertises support, and returns to JPEG if
H.264 encoding fails.

## Quality behavior

The Agent captures at up to `REMOTEX_VIDEO_FPS` (default `30`, range `1..=30`).
Controller feedback is sent at most once per second. A hysteresis controller
uses decoder delay and queue pressure to select one of these bounded profiles:

| Profile | Maximum resolution | FPS | Target bitrate |
| --- | ---: | ---: | ---: |
| Poor | 1280×720 | 10 | 800 kbps |
| Medium | 1600×900 | 20 | 2 Mbps |
| Good | 1920×1080 | 30 | 4 Mbps |

Three consecutive poor reports reduce quality; six consecutive good reports
increase it. Resolution changes rebuild the encoder and force an independently
decodable key frame. The maximum Agent FPS remains an upper bound for every
profile.

The Controller keeps one decoder for the whole stream, converts decoded frames
to RGBA for the Tauri canvas, and displays codec, resolution, FPS, bitrate, and
an end-to-end latency estimate. Frame metadata also records capture, encode,
decode, and key-frame information. The E2E estimate assumes reasonably aligned
host clocks and is diagnostic rather than an authentication input.

## Dirty rectangles and hardware codecs

The current capture abstraction returns compact full frames and does not expose
DXGI dirty/move rectangles. M10 therefore relies on H.264 inter-frame
compression for static desktops. Dirty-rectangle metadata and NVENC/QSV/AMF
implementations can be added behind the existing `VideoEncoder` trait without
changing transport code.

## Compatibility and limits

JPEG and WebP remain accepted by the decoder. The Agent sends JPEG until codec
negotiation completes, so older Controllers retain the M3 path. Encoded payloads
are capped at 8 MiB and decoded dimensions at 3840×2160 before allocation.
Malformed metadata, invalid dimensions, empty/oversized payloads, and decoder
errors are returned as typed failures rather than panics.

