import { useEffect, useState } from "react";
import type { VideoStats } from "./types";
import { VIDEO_STATS_EVENT } from "./types";

const initial: VideoStats = { received: 0, rendered: 0, dropped: 0, renderFps: 0, decodeLatencyMs: 0, endToEndLatencyMs: 0, width: 0, height: 0, codec: "-" };

export function DiagnosticsOverlay({ connectionState }: { connectionState: string }) {
  const [visible, setVisible] = useState(false);
  const [stats, setStats] = useState(initial);
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.ctrlKey && event.shiftKey && event.code === "KeyD") {
        event.preventDefault(); setVisible((value) => !value);
      }
    };
    const onStats = (event: Event) => setStats((event as CustomEvent<VideoStats>).detail);
    window.addEventListener("keydown", onKey);
    window.addEventListener(VIDEO_STATS_EVENT, onStats);
    return () => { window.removeEventListener("keydown", onKey); window.removeEventListener(VIDEO_STATS_EVENT, onStats); };
  }, []);
  if (!visible) return null;
  return <aside className="diagnostics-overlay" aria-label="Performance diagnostics">
    <header><strong>Diagnostics</strong><span>Ctrl+Shift+D</span></header>
    <dl>
      <div><dt>Connection</dt><dd>{connectionState}</dd></div>
      <div><dt>Video</dt><dd>{stats.width}×{stats.height} {stats.codec}</dd></div>
      <div><dt>Rendered</dt><dd>{stats.renderFps} FPS</dd></div>
      <div><dt>Frames</dt><dd>{stats.received} / {stats.rendered} / {stats.dropped}</dd></div>
      <div><dt>Decode</dt><dd>{stats.decodeLatencyMs} ms</dd></div>
      <div><dt>End to end</dt><dd>{stats.endToEndLatencyMs} ms</dd></div>
    </dl>
    <small>Received / rendered / dropped. No session content is logged.</small>
  </aside>;
}

