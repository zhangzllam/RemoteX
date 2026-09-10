import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { ConnectionMetrics, VideoStats } from "./types";
import { CONNECTION_METRICS_EVENT, VIDEO_STATS_EVENT } from "./types";

const initial: VideoStats = { received: 0, rendered: 0, dropped: 0, renderFps: 0, bitrateBps: 0, captureLatencyMs: 0, encodeLatencyMs: 0, decodeLatencyMs: 0, renderLatencyMs: null, frameAgeMs: null, width: 0, height: 0, codec: "-" };
const initialNetwork: ConnectionMetrics = { rttMs: null, packetLossPerMille: null, sendQueuePercent: null, sendQueueEstimated: false };

export function DiagnosticsOverlay({ connectionState, connectionPath, transport }: { connectionState: string; connectionPath?: "direct" | "relay"; transport?: "quic" }) {
  const [visible, setVisible] = useState(false);
  const [stats, setStats] = useState(initial);
  const [network, setNetwork] = useState(initialNetwork);
  useEffect(() => {
    let disposed = false;
    let stopNetwork: (() => void) | undefined;
    const onKey = (event: KeyboardEvent) => {
      if (event.ctrlKey && event.shiftKey && event.code === "KeyD") {
        event.preventDefault(); setVisible((value) => !value);
      }
    };
    const onStats = (event: Event) => setStats((event as CustomEvent<VideoStats>).detail);
    const onToggle = () => setVisible((value) => !value);
    window.addEventListener("keydown", onKey);
    window.addEventListener(VIDEO_STATS_EVENT, onStats);
    window.addEventListener("remotex-toggle-diagnostics", onToggle);
    void listen<ConnectionMetrics>(CONNECTION_METRICS_EVENT, ({ payload }) => setNetwork(payload))
      .then((stop) => { if (disposed) stop(); else stopNetwork = stop; });
    return () => {
      disposed = true;
      stopNetwork?.();
      window.removeEventListener("keydown", onKey);
      window.removeEventListener(VIDEO_STATS_EVENT, onStats);
      window.removeEventListener("remotex-toggle-diagnostics", onToggle);
    };
  }, []);
  if (!visible) return null;
  return <aside className="diagnostics-overlay" aria-label="Performance diagnostics">
    <header><strong>Diagnostics</strong><span>Ctrl+Shift+D</span></header>
    <dl>
      <div><dt>Connection</dt><dd>{connectionPath === "direct" ? "P2P Direct" : connectionPath === "relay" ? "Relay" : connectionState}</dd></div>
      <div><dt>Transport</dt><dd>{transport?.toUpperCase() ?? "—"}</dd></div>
      <div><dt>Video</dt><dd>{stats.width}×{stats.height} {stats.codec}</dd></div>
      <div><dt>Rendered</dt><dd>{stats.renderFps} FPS</dd></div>
      <div><dt>Bitrate</dt><dd>{stats.bitrateBps ? `${(stats.bitrateBps / 1_000_000).toFixed(1)} Mbps` : "—"}</dd></div>
      <div><dt>RTT</dt><dd>{network.rttMs === null ? "—" : `${network.rttMs} ms`}</dd></div>
      <div><dt>Packet loss</dt><dd>{network.packetLossPerMille === null ? "—" : `${(network.packetLossPerMille / 10).toFixed(1)}%`}</dd></div>
      <div><dt>Send queue</dt><dd>{network.sendQueuePercent === null ? "—" : `${network.sendQueuePercent}%${network.sendQueueEstimated ? " ~" : ""}`}</dd></div>
      <div><dt>Frames</dt><dd>{stats.received} / {stats.rendered} / {stats.dropped}</dd></div>
      <div><dt>Decode</dt><dd>{stats.decodeLatencyMs} ms</dd></div>
      <div><dt>Capture</dt><dd>{stats.captureLatencyMs} ms</dd></div>
      <div><dt>Encode</dt><dd>{stats.encodeLatencyMs} ms</dd></div>
      <div><dt>Render</dt><dd>{stats.renderLatencyMs === null ? "—" : `${stats.renderLatencyMs.toFixed(1)} ms`}</dd></div>
      <div><dt>Frame age</dt><dd>—</dd></div>
    </dl>
    <small>Received / rendered / dropped. No session content is logged.</small>
  </aside>;
}
