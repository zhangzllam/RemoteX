import { useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { VideoFrame, VideoStats } from "./types";
import { VIDEO_STATS_EVENT } from "./types";

type Props = {
  connected: boolean;
  statusMessage: string;
  onFrame: (frame: VideoFrame) => void;
};

const emptyStats: VideoStats = {
  received: 0, rendered: 0, dropped: 0, renderFps: 0,
  decodeLatencyMs: 0, endToEndLatencyMs: 0, width: 0, height: 0, codec: "-",
};

export function RemoteVideoRenderer({ connected, statusMessage, onFrame }: Props) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const latestFrame = useRef<VideoFrame | null>(null);
  const animationFrame = useRef<number | null>(null);
  const stats = useRef<VideoStats>({ ...emptyStats });
  const onFrameRef = useRef(onFrame);
  const renderedThisWindow = useRef(0);
  const [hasFrame, setHasFrame] = useState(false);
  const [summary, setSummary] = useState<VideoStats>({ ...emptyStats });
  useEffect(() => { onFrameRef.current = onFrame; }, [onFrame]);

  useEffect(() => {
    let disposed = false;
    const acceptFrame = (payload: VideoFrame) => {
      stats.current.received += 1;
      if (latestFrame.current) stats.current.dropped += 1;
      stats.current.decodeLatencyMs = payload.decodeLatencyMs;
      stats.current.endToEndLatencyMs = payload.endToEndLatencyMs;
      stats.current.width = payload.width;
      stats.current.height = payload.height;
      stats.current.codec = payload.codec;
      latestFrame.current = payload;
      if (animationFrame.current === null) animationFrame.current = requestAnimationFrame(renderLatest);
    };
    const renderLatest = () => {
      animationFrame.current = null;
      const frame = latestFrame.current;
      latestFrame.current = null;
      if (!frame || disposed) return;
      const canvas = canvasRef.current;
      const context = canvas?.getContext("2d", { alpha: false });
      if (!canvas || !context) return;
      try {
        if (frame.mimeType === "application/x-remotex-rgba") {
          const rgba: Uint8ClampedArray<ArrayBuffer> = frame.bytes
            ? copyBytes(frame.bytes)
            : decodeBase64(frame.data);
          if (rgba.length !== frame.width * frame.height * 4) return;
          if (canvas.width !== frame.width) canvas.width = frame.width;
          if (canvas.height !== frame.height) canvas.height = frame.height;
          context.putImageData(new ImageData(rgba, frame.width, frame.height), 0, 0);
          stats.current.rendered += 1;
          renderedThisWindow.current += 1;
          setHasFrame(true);
          onFrameRef.current(frame);
        }
      } finally {
        if (latestFrame.current && animationFrame.current === null) {
          animationFrame.current = requestAnimationFrame(renderLatest);
        }
      }
    };
    const channel = new Channel<Uint8Array | number[] | ArrayBuffer>();
    channel.onmessage = (packet) => { const frame = parseVideoPacket(packet); if (frame) acceptFrame(frame); };
    void invoke("subscribe_video", { channel });
    const unlisten = listen<VideoFrame>("video-frame", ({ payload }) => acceptFrame(payload));
    const interval = window.setInterval(() => {
      stats.current.renderFps = renderedThisWindow.current * 2;
      renderedThisWindow.current = 0;
      const next = { ...stats.current };
      setSummary(next);
      window.dispatchEvent(new CustomEvent<VideoStats>(VIDEO_STATS_EVENT, { detail: next }));
    }, 500);
    return () => {
      disposed = true;
      void unlisten.then((stop) => stop());
      window.clearInterval(interval);
      if (animationFrame.current !== null) cancelAnimationFrame(animationFrame.current);
    };
  }, []);

  useEffect(() => {
    if (!connected) {
      latestFrame.current = null;
      stats.current = { ...emptyStats };
      renderedThisWindow.current = 0;
      setHasFrame(false);
      setSummary({ ...emptyStats });
    }
  }, [connected]);

  return <>
    <canvas ref={canvasRef} aria-label="Remote desktop video" className={hasFrame ? "" : "video-pending"} />
    {!hasFrame && <div className="empty screen-empty">{connected ? <><span className="spinner large" /><strong>Waiting for the first frame</strong><small>{statusMessage}</small></> : <><strong>Session ended</strong><small>Return Home to connect again.</small></>}</div>}
    {hasFrame && <div className="telemetry">{summary.width}×{summary.height} · {summary.codec} · {summary.renderFps} FPS · {summary.endToEndLatencyMs} ms</div>}
  </>;
}

function copyBytes(bytes: Uint8Array): Uint8ClampedArray<ArrayBuffer> {
  const copy = new Uint8ClampedArray(new ArrayBuffer(bytes.byteLength));
  copy.set(bytes);
  return copy;
}

function decodeBase64(data: string): Uint8ClampedArray<ArrayBuffer> {
  const binary = window.atob(data);
  const rgba = new Uint8ClampedArray(new ArrayBuffer(binary.length));
  for (let index = 0; index < binary.length; index += 1) rgba[index] = binary.charCodeAt(index);
  return rgba;
}

function parseVideoPacket(value: Uint8Array | number[] | ArrayBuffer): VideoFrame | null {
  const bytes = value instanceof Uint8Array ? value : new Uint8Array(value);
  if (bytes.byteLength < 68 || bytes[0] !== 0x52 || bytes[1] !== 0x58 || bytes[2] !== 0x56 || bytes[3] !== 0x46 || bytes[4] !== 1) return null;
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const width = view.getUint32(40, true);
  const height = view.getUint32(44, true);
  const rgba = bytes.subarray(68);
  if (rgba.byteLength !== width * height * 4) return null;
  const codec = bytes[5] === 1 ? "H.264" : bytes[5] === 2 ? "JPEG" : "WebP";
  return {
    sequence: Number(view.getBigUint64(8, true)), frameId: Number(view.getBigUint64(16, true)),
    sourceTimestampMs: Number(view.getBigUint64(24, true)), endToEndLatencyMs: Number(view.getBigUint64(32, true)),
    width, height, framesPerSecond: view.getUint32(48, true), bitrateBps: view.getUint32(52, true),
    captureLatencyMs: view.getUint32(56, true), encodeLatencyMs: view.getUint32(60, true), decodeLatencyMs: view.getUint32(64, true),
    codec, keyFrame: bytes[6] === 1, mimeType: "application/x-remotex-rgba", data: "", bytes: rgba,
  };
}
