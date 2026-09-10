# Performance and diagnostics

## Video path

```text
network -> authenticate/decrypt -> persistent decoder -> RGBA
        -> Tauri binary Channel -> latest-frame slot -> requestAnimationFrame
        -> canvas
```

The network, decryption, and decoder remain in Rust. The WebView receives a
binary packet and paints at most one current frame per browser animation turn.
If another frame arrives before painting, it replaces the pending frame and the
dropped-frame counter increments. This is intentional latency control, not data
loss in a reliable document stream.

Press `Ctrl+Shift+D` to show or hide diagnostics. The overlay is local-only and
does not log session pixels, clipboard content, keys, file paths, credentials,
or secrets.

Metrics:

- **Received**: valid packets accepted by the WebView.
- **Rendered**: packets copied to the canvas.
- **Dropped**: pending packets replaced before canvas rendering.
- **Render FPS**: canvas updates during the last half-second window.
- **Decode**: Rust decoder time reported with the frame.
- **End to end**: source timestamp to Rust IPC handoff.

## Targets and triage

- Balanced mode should sustain its negotiated 20 FPS without a growing queue.
- At 30 FPS, occasional latest-frame drops are acceptable when input remains
  responsive and end-to-end latency does not trend upward.
- Sustained drop rates over 20%, decode above the frame budget, or increasing
  end-to-end latency indicate that quality should step down.
- Profile capture, codec, IPC, canvas, and network separately; do not infer a
  network problem from render FPS alone.

Use `pnpm --dir apps/desktop/ui benchmark:video` for the reproducible synthetic
IPC comparison. Use a real authorized session and the diagnostics overlay for
end-to-end evaluation at 1280x720, 1920x1080, 10/20/30 FPS, direct, LAN, and
relayed paths.

## Resource controls retained

Protocol and video frame limits, bounded transport queues, one-in-flight file
chunks, clipboard caps, terminal limits, and session cleanup remain unchanged.
The latest-frame slot is bounded to exactly one pending WebView frame.

