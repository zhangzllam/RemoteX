export type VideoFrame = {
  sequence: number;
  frameId: number;
  width: number;
  height: number;
  framesPerSecond: number;
  bitrateBps: number;
  sourceTimestampMs: number;
  captureLatencyMs: number;
  encodeLatencyMs: number;
  decodeLatencyMs: number;
  codec: string;
  keyFrame: boolean;
  mimeType: string;
  data: string;
  bytes?: Uint8Array;
};

export type VideoStats = {
  received: number;
  rendered: number;
  dropped: number;
  renderFps: number;
  bitrateBps: number;
  captureLatencyMs: number;
  encodeLatencyMs: number;
  decodeLatencyMs: number;
  renderLatencyMs: number | null;
  frameAgeMs: number | null;
  width: number;
  height: number;
  codec: string;
};

export type ConnectionMetrics = {
  rttMs: number | null;
  packetLossPerMille: number | null;
  sendQueuePercent: number | null;
  sendQueueEstimated: boolean;
};

export type ServerConfig = {
  controlServerUrl: string;
  relayAddress: string;
  relayServerName: string;
  stunAddress: string;
  caCertificatePath: string;
  configured: boolean;
};

export type ServerConfigCandidate = {
  source: string;
  config: ServerConfig;
};

export type ServerConfigState = {
  config: ServerConfig;
  needsSetup: boolean;
  migrationConflict: boolean;
  candidates: ServerConfigCandidate[];
};

export type ServerCheckResult = {
  ok: boolean;
  endpoint: string;
  message: string;
};

export type AgentSettings = {
  remoteAccessEnabled: boolean;
  serverUrl: string;
  deviceName: string;
  caCertificatePath: string;
  allowInput: boolean;
  allowClipboard: boolean;
  allowFileUpload: boolean;
  allowFileDownload: boolean;
  fileRoots: string;
  unattendedAccess: boolean;
  unattendedSecret: string;
  secretConfigured: boolean;
  startWithWindows: boolean;
  videoQuality: "auto" | "quality" | "balanced" | "lowBandwidth";
};

export type AgentRuntimeStatus = {
  running: boolean;
  processId: number | null;
  deviceId: string | null;
  state: string;
  sessionId: string | null;
  startWithWindows: boolean;
};

export const emptyServerConfig: ServerConfig = {
  controlServerUrl: "",
  relayAddress: "",
  relayServerName: "",
  stunAddress: "",
  caCertificatePath: "",
  configured: false,
};

export const VIDEO_STATS_EVENT = "remotex-video-stats";
export const CONNECTION_METRICS_EVENT = "connection-metrics";
