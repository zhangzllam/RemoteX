# Video IPC benchmark

Measured on 2026-08-12 on Windows x64 with Node 24.14.0. Run the repeatable
synthetic benchmark with:

```powershell
pnpm --dir apps/desktop/ui benchmark:video
```

The benchmark models one second of decoded RGBA frames. `base64` performs the
v1.1 encode, decode, and typed-array copy. `bytes` models a binary channel and
typed-array copy. It does not include capture, H.264 decoding, WebView painting,
network delay, or GPU composition, so these numbers compare IPC preparation
cost rather than end-to-end session performance.

| Scenario | Path | Mean/frame | p95/frame | CPU | Peak RSS delta | Synthetic processing |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 720p15 | base64 | 2.58 ms | 5.42 ms | 0 ms* | 94.70 MiB | 386 FPS |
| 720p15 | bytes | 0.62 ms | 1.01 ms | 0 ms* | 52.80 MiB | 1,605 FPS |
| 1080p30 | base64 | 6.22 ms | 9.88 ms | 249 ms | 32.63 MiB | 160 FPS |
| 1080p30 | bytes | 1.74 ms | 3.83 ms | 32 ms | 15.84 MiB | 574 FPS |
| 1080p60 | base64 | 5.86 ms | 8.61 ms | 297 ms | 68.62 MiB | 170 FPS |
| 1080p60 | bytes | 1.73 ms | 3.65 ms | 108 ms | 0 MiB* | 576 FPS |

`*` Process counters are coarse and garbage collection makes RSS deltas noisy;
repeat several times before comparing machines.

## Decision

Tauri events support JSON payloads only. Tauri documents optimized array-buffer
responses and recommends channels for ordered, high-throughput streams. RemoteX
v1.2 therefore uses a `Channel<Vec<u8>>` with a fixed 68-byte metadata header
followed by RGBA bytes. The old base64 event remains only as a compatibility
fallback when no channel has subscribed. See the official
[Tauri IPC and Channel documentation](https://v2.tauri.app/develop/calling-rust/).

The renderer keeps only the newest pending frame. One `requestAnimationFrame`
callback consumes it, so a slow WebView drops stale frames instead of building
an unbounded queue. Video receipt no longer writes each frame into the root
React component state. The diagnostics overlay reports received, rendered, and
dropped counts, render FPS, decode latency, and end-to-end latency.
