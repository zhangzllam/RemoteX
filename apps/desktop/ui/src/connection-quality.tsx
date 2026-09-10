import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useI18n } from "./i18n";
import type { ConnectionMetrics, VideoStats } from "./types";
import { CONNECTION_METRICS_EVENT, VIDEO_STATS_EVENT } from "./types";

type Quality = "Excellent" | "Good" | "Fair" | "Poor" | "Measuring";

const emptyNetwork: ConnectionMetrics = {
  rttMs: null,
  packetLossPerMille: null,
  sendQueuePercent: null,
  sendQueueEstimated: false,
};

export function ConnectionQualityIndicator({ path }: { path?: "direct" | "relay" }) {
  const { t } = useI18n();
  const [network, setNetwork] = useState(emptyNetwork);
  const [dropRatio, setDropRatio] = useState<number | null>(null);
  useEffect(() => {
    let disposed = false;
    let stopNetwork: (() => void) | undefined;
    const onStats = (event: Event) => {
      const stats = (event as CustomEvent<VideoStats>).detail;
      const total = stats.rendered + stats.dropped;
      setDropRatio(total ? stats.dropped / total : null);
    };
    window.addEventListener(VIDEO_STATS_EVENT, onStats);
    void listen<ConnectionMetrics>(CONNECTION_METRICS_EVENT, ({ payload }) => setNetwork(payload))
      .then((stop) => { if (disposed) stop(); else stopNetwork = stop; });
    return () => {
      disposed = true;
      stopNetwork?.();
      window.removeEventListener(VIDEO_STATS_EVENT, onStats);
    };
  }, []);
  const quality = useMemo(() => connectionQuality(network, dropRatio), [network, dropRatio]);
  const pathLabel = path === "direct" ? t("P2P Direct", "P2P 直连") : path === "relay" ? t("Relay", "服务器中继") : t("Negotiating", "正在协商");
  const qualityLabel = {
    Excellent: t("Excellent", "优秀"),
    Good: t("Good", "良好"),
    Fair: t("Fair", "一般"),
    Poor: t("Poor", "较差"),
    Measuring: t("Measuring", "测量中"),
  }[quality];
  return <button
    type="button"
    className={`connection-path-pill path-${path ?? "negotiating"} quality-${quality.toLowerCase()}`}
    aria-label={`${t("Connection quality", "连接质量")}: ${qualityLabel}`}
    onClick={() => window.dispatchEvent(new Event("remotex-toggle-diagnostics"))}
  ><i />{pathLabel}<span aria-hidden="true">·</span>{qualityLabel}</button>;
}

export function connectionQuality(network: ConnectionMetrics, dropRatio: number | null): Quality {
  const available = [network.rttMs, network.packetLossPerMille, network.sendQueuePercent, dropRatio]
    .filter((value) => value !== null).length;
  if (available < 2) return "Measuring";
  const rtt = network.rttMs ?? 0;
  const loss = (network.packetLossPerMille ?? 0) / 1_000;
  const queue = (network.sendQueuePercent ?? 0) / 100;
  const drops = dropRatio ?? 0;
  if (rtt >= 250 || loss >= 0.08 || queue >= 0.8 || drops >= 0.15) return "Poor";
  if (rtt >= 140 || loss >= 0.03 || queue >= 0.6 || drops >= 0.08) return "Fair";
  if (rtt >= 80 || loss >= 0.01 || queue >= 0.35 || drops >= 0.03) return "Good";
  return "Excellent";
}
