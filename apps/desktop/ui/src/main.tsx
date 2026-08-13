import {
  FormEvent,
  KeyboardEvent as ReactKeyboardEvent,
  PointerEvent as ReactPointerEvent,
  type ReactNode,
  WheelEvent as ReactWheelEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { createRoot, type Root } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { AppUpdatePanel, AppUpdaterSettings, useAppUpdater } from "./app-updater";
import { useAppearance, type AppearancePreference } from "./appearance";
import { DiagnosticsOverlay } from "./diagnostics-overlay";
import { friendlyError, friendlyFailure } from "./friendly-errors";
import { HomePage, type HomeRecentDevice } from "./home-page";
import { I18nProvider, useI18n, type AppLanguage } from "./i18n";
import { remoteXApi } from "./remote-api";
import { RemoteVideoRenderer } from "./remote-video-renderer";
import { SetupWizard } from "./setup-wizard";
import type { AgentRuntimeStatus, AgentSettings, ServerConfig, ServerConfigState, VideoFrame } from "./types";
import { emptyServerConfig } from "./types";
import { Icon, StatusPill, Toggle, type IconName } from "./ui";
import "./design-tokens.css";
import "./components.css";
import "./layout.css";
import "./session.css";

type ConnectionStatus = {
  state: string;
  message: string;
};

type TerminalEvent =
  | { kind: "output"; terminalId: string; data: string }
  | { kind: "closed"; terminalId: string; exitCode: number | null }
  | { kind: "error"; terminalId: string | null; message: string };

type SystemSnapshot = {
  hostname: string;
  operatingSystem: string;
  kernelVersion: string;
  cpuModel: string;
  cpuCount: number;
  totalMemoryBytes: number;
  usedMemoryBytes: number;
  uptimeSeconds: number;
  disks: { name: string; mountPoint: string; totalBytes: number; availableBytes: number }[];
  networkInterfaces: { name: string; receivedBytes: number; transmittedBytes: number }[];
  gpus: { name: string; utilizationPercent: number | null; memoryUsedBytes: number | null; memoryTotalBytes: number | null; temperatureCelsius: number | null }[];
};

const ANSI_COLORS: Record<number, string> = {
  30: "#1b1f27", 31: "#ff6b6b", 32: "#78dba9", 33: "#ffd166",
  34: "#6fa8ff", 35: "#d58cff", 36: "#67d9df", 37: "#e6edf7",
  90: "#7d8798", 91: "#ff8e8e", 92: "#9bf0c3", 93: "#ffe29a",
  94: "#94bdff", 95: "#e3b0ff", 96: "#91edf1", 97: "#ffffff",
};
const IS_TAURI = typeof (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ === "object";

function renderAnsi(text: string): ReactNode[] {
  const output: ReactNode[] = [];
  const pattern = /\u001b\[([0-9;?]*)([A-Za-z])/g;
  let offset = 0;
  let color: string | undefined;
  let bold = false;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(text)) !== null) {
    if (match.index > offset) {
      output.push(<span key={`${offset}-${match.index}`} style={{ color, fontWeight: bold ? 700 : undefined }}>{text.slice(offset, match.index)}</span>);
    }
    if (match[2] === "m") {
      for (const value of (match[1] || "0").split(";").map(Number)) {
        if (value === 0) { color = undefined; bold = false; }
        else if (value === 1) bold = true;
        else if (value === 22) bold = false;
        else if (value === 39) color = undefined;
        else if (ANSI_COLORS[value]) color = ANSI_COLORS[value];
      }
    }
    offset = pattern.lastIndex;
  }
  if (offset < text.length) output.push(<span key={offset} style={{ color, fontWeight: bold ? 700 : undefined }}>{text.slice(offset)}</span>);
  return output;
}

type ConnectRequest = {
  controlServerUrl: string;
  deviceId: string;
  controllerName: string;
  unattendedSecret: string;
  relayAddress: string;
  serverName: string;
  caCertificatePath: string;
  sessionId: string;
  tokenHex: string;
  endToEndKeyHex: string;
  clipboardEnabled: boolean;
  fileUploadEnabled: boolean;
  fileDownloadEnabled: boolean;
};

type Page = "home" | "devices" | "files" | "settings" | "about" | "session";
type SettingsSection = "general" | "appearance" | "remote" | "permissions" | "video" | "network" | "security" | "advanced";
type AllowedFolder = { label: string; path: string };
type RecentDevice = HomeRecentDevice;

const PRIMARY_NAV_ITEMS: { page: Exclude<Page, "session">; label: string; icon: IconName }[] = [
  { page: "home", label: "Home", icon: "home" },
  { page: "devices", label: "Devices", icon: "devices" },
  { page: "files", label: "Files", icon: "files" },
];
const SECONDARY_NAV_ITEMS: { page: Exclude<Page, "session">; label: string; icon: IconName }[] = [
  { page: "settings", label: "Settings", icon: "settings" },
  { page: "about", label: "About", icon: "about" },
];
const NAV_ITEMS = [...PRIMARY_NAV_ITEMS, ...SECONDARY_NAV_ITEMS];
const SETTINGS_SECTIONS: { id: SettingsSection; label: string; icon: IconName }[] = [
  { id: "general", label: "General", icon: "settings" }, { id: "appearance", label: "Appearance", icon: "sun" }, { id: "remote", label: "Remote Access", icon: "power" },
  { id: "permissions", label: "Permissions", icon: "shield" }, { id: "video", label: "Video", icon: "monitor" },
  { id: "network", label: "Network", icon: "wifi" }, { id: "security", label: "Security", icon: "server" },
];
const APPEARANCE_OPTIONS: { id: AppearancePreference; label: string; detail: string; icon: IconName }[] = [
  { id: "system", label: "System", detail: "Match Windows", icon: "monitor" },
  { id: "light", label: "Light", detail: "Bright surfaces", icon: "sun" },
  { id: "dark", label: "Dark", detail: "Dim surfaces", icon: "moon" },
];
function parseFolders(value: string): AllowedFolder[] {
  return value.split(";").map((item) => item.trim()).filter(Boolean).map((item) => {
    const separator = item.indexOf("=");
    if (separator < 0) return { label: item.split(/[\\/]/).filter(Boolean).at(-1) ?? "Folder", path: item };
    return { label: item.slice(0, separator).trim() || "Folder", path: item.slice(separator + 1).trim() };
  }).filter((folder) => Boolean(folder.path));
}
function serializeFolders(folders: AllowedFolder[]): string {
  return folders.map((folder) => `${folder.label.replace(/[;=]/g, "").trim() || "Folder"}=${folder.path}`).join(";");
}
function formatDeviceId(value: string | null | undefined): string {
  if (!value) return "—";
  const digits = value.replace(/\D/g, "");
  return digits.length === 9 ? `${digits.slice(0, 3)} ${digits.slice(3, 6)} ${digits.slice(6)}` : value;
}
function loadRecentDevices(): RecentDevice[] {
  try { return (JSON.parse(localStorage.getItem("remotex-recent-devices") ?? "[]") as RecentDevice[]).filter((item) => /^\d{9}$/.test(item.deviceId)).slice(0, 5); }
  catch { return []; }
}
type RemoteFileEntry = {
  name: string;
  path: string;
  entryType: "file" | "directory";
  size: number;
  modifiedMs: number | null;
};

type TransferProgress = {
  transferId: string;
  direction: string;
  transferred: number;
  total: number;
  state: string;
};

type FileEvent =
  | { kind: "directory"; path: string; entries: RemoteFileEntry[] }
  | { kind: "directoryCreated"; path: string }
  | ({ kind: "progress" } & TransferProgress)
  | { kind: "error"; transferId: string | null; message: string };

type RemoteMouseButton = "left" | "right" | "middle";

type MouseInputRequest =
  | { kind: "move"; displayId: string | null; x: number; y: number }
  | { kind: "buttonDown"; button: RemoteMouseButton; x: number; y: number }
  | { kind: "buttonUp"; button: RemoteMouseButton }
  | { kind: "wheel"; horizontalDelta: number; verticalDelta: number };

const REMOTE_KEY_BY_CODE = {
  KeyA: "keyA",
  KeyB: "keyB",
  KeyC: "keyC",
  KeyD: "keyD",
  KeyE: "keyE",
  KeyF: "keyF",
  KeyG: "keyG",
  KeyH: "keyH",
  KeyI: "keyI",
  KeyJ: "keyJ",
  KeyK: "keyK",
  KeyL: "keyL",
  KeyM: "keyM",
  KeyN: "keyN",
  KeyO: "keyO",
  KeyP: "keyP",
  KeyQ: "keyQ",
  KeyR: "keyR",
  KeyS: "keyS",
  KeyT: "keyT",
  KeyU: "keyU",
  KeyV: "keyV",
  KeyW: "keyW",
  KeyX: "keyX",
  KeyY: "keyY",
  KeyZ: "keyZ",
  Digit0: "digit0",
  Digit1: "digit1",
  Digit2: "digit2",
  Digit3: "digit3",
  Digit4: "digit4",
  Digit5: "digit5",
  Digit6: "digit6",
  Digit7: "digit7",
  Digit8: "digit8",
  Digit9: "digit9",
  F1: "f1",
  F2: "f2",
  F3: "f3",
  F4: "f4",
  F5: "f5",
  F6: "f6",
  F7: "f7",
  F8: "f8",
  F9: "f9",
  F10: "f10",
  F11: "f11",
  F12: "f12",
  Enter: "enter",
  Escape: "escape",
  Tab: "tab",
  Backspace: "backspace",
  Delete: "delete",
  Insert: "insert",
  Home: "home",
  End: "end",
  PageUp: "pageUp",
  PageDown: "pageDown",
  ArrowLeft: "arrowLeft",
  ArrowRight: "arrowRight",
  ArrowUp: "arrowUp",
  ArrowDown: "arrowDown",
  Space: "space",
  ShiftLeft: "shiftLeft",
  ShiftRight: "shiftRight",
  ControlLeft: "controlLeft",
  ControlRight: "controlRight",
  AltLeft: "altLeft",
  AltRight: "altRight",
  MetaLeft: "superLeft",
  MetaRight: "superRight",
} as const;

type RemoteKeyCode = (typeof REMOTE_KEY_BY_CODE)[keyof typeof REMOTE_KEY_BY_CODE];

type KeyboardInputRequest =
  | { kind: "keyDown"; key: RemoteKeyCode }
  | { kind: "keyUp"; key: RemoteKeyCode };

type UnitPoint = { x: number; y: number };

const defaultRequest: ConnectRequest = {
  controlServerUrl: "",
  deviceId: "",
  controllerName: "RemoteX Desktop",
  unattendedSecret: "",
  relayAddress: "",
  serverName: "",
  caCertificatePath: "",
  sessionId: "",
  tokenHex: "",
  endToEndKeyHex: "",
  clipboardEnabled: false,
  fileUploadEnabled: false,
  fileDownloadEnabled: false,
};

const defaultAgentSettings: AgentSettings = {
  remoteAccessEnabled: false,
  serverUrl: "",
  deviceName: "Windows PC",
  caCertificatePath: "",
  allowInput: false,
  allowClipboard: false,
  allowFileUpload: false,
  allowFileDownload: false,
  fileRoots: "",
  unattendedAccess: false,
  unattendedSecret: "",
  secretConfigured: false,
  startWithWindows: false,
  videoQuality: "balanced",
};

function loadControllerSettings(): ConnectRequest {
  try {
    const stored = JSON.parse(window.localStorage.getItem("remotex-controller-settings") ?? "{}") as Partial<ConnectRequest>;
    return {
      ...defaultRequest,
      controlServerUrl: stored.controlServerUrl ?? defaultRequest.controlServerUrl,
      controllerName: stored.controllerName ?? defaultRequest.controllerName,
      caCertificatePath: stored.caCertificatePath ?? defaultRequest.caCertificatePath,
      clipboardEnabled: stored.clipboardEnabled ?? false,
      fileUploadEnabled: stored.fileUploadEnabled ?? false,
      fileDownloadEnabled: stored.fileDownloadEnabled ?? false,
    };
  } catch {
    return defaultRequest;
  }
}

function parentPath(path: string): string {
  if (path === "/") return "/";
  const end = path.lastIndexOf("/");
  return end <= 0 ? "/" : path.slice(0, end);
}

function childPath(parent: string, name: string): string {
  return parent === "/" ? `/${name}` : `${parent}/${name}`;
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KiB`;
  if (value < 1024 * 1024 * 1024) return `${(value / 1024 / 1024).toFixed(1)} MiB`;
  return `${(value / 1024 / 1024 / 1024).toFixed(1)} GiB`;
}

function mouseButton(button: number): RemoteMouseButton | null {
  switch (button) {
    case 0:
      return "left";
    case 1:
      return "middle";
    case 2:
      return "right";
    default:
      return null;
  }
}

function wheelDelta(value: number): number {
  const truncated = Math.trunc(value);
  return Math.max(-2_147_483_647, Math.min(2_147_483_647, truncated));
}

function App() {
  const { language, setLanguage, t } = useI18n();
  const { preference: appearance, setPreference: setAppearance } = useAppearance();
  const [request, setRequest] = useState(loadControllerSettings);
  const [status, setStatus] = useState<ConnectionStatus>({
    state: "disconnected",
    message: "",
  });
  const [page, setPage] = useState<Page>("home");
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("general");
  const [recentDevices, setRecentDevices] = useState<RecentDevice[]>(loadRecentDevices);
  const [copied, setCopied] = useState(false);
  const [sessionSeconds, setSessionSeconds] = useState(0);
  const [uiError, setUiError] = useState("");
  const [saving, setSaving] = useState(false);
  const [hasVideoFrame, setHasVideoFrame] = useState(false);
  const [serverConfig, setServerConfig] = useState<ServerConfig>(emptyServerConfig);
  const [serverConfigState, setServerConfigState] = useState<ServerConfigState | null>(null);
  const [showSetup, setShowSetup] = useState(false);
  const [networkMessage, setNetworkMessage] = useState("");
  const [filePath, setFilePath] = useState("/");
  const [fileEntries, setFileEntries] = useState<RemoteFileEntry[]>([]);
  const [fileError, setFileError] = useState("");
  const [newFolderName, setNewFolderName] = useState("");
  const [uploadLocalPath, setUploadLocalPath] = useState("");
  const [uploadDestinationPath, setUploadDestinationPath] = useState("");
  const [downloadSourcePath, setDownloadSourcePath] = useState("");
  const [downloadLocalPath, setDownloadLocalPath] = useState("");
  const [resumeTransferId, setResumeTransferId] = useState("");
  const [transfers, setTransfers] = useState<Record<string, TransferProgress>>({});
  const [terminalId, setTerminalId] = useState("");
  const [terminalOutput, setTerminalOutput] = useState("");
  const [terminalInput, setTerminalInput] = useState("");
  const [systemInfo, setSystemInfo] = useState<SystemSnapshot | null>(null);
  const [agentSettings, setAgentSettings] = useState(defaultAgentSettings);
  const [agentRuntime, setAgentRuntime] = useState<AgentRuntimeStatus | null>(null);
  const [agentMessage, setAgentMessage] = useState("");
  const settingsRef = useRef<HTMLElement>(null);
  const screenRef = useRef<HTMLDivElement>(null);
  const frameRef = useRef<VideoFrame | null>(null);
  const pressedButtons = useRef(new Set<RemoteMouseButton>());
  const pressedKeys = useRef(new Set<RemoteKeyCode>());
  const pendingMove = useRef<UnitPoint | null>(null);
  const moveAnimationFrame = useRef<number | null>(null);
  const inputChain = useRef<Promise<void>>(Promise.resolve());
  const appUpdater = useAppUpdater({
    activeRemoteSession: status.state === "connected" || Boolean(agentRuntime?.sessionId),
    onAgentStopped: () => setAgentRuntime((current) => current ? { ...current, running: false, processId: null, state: "stopped", sessionId: null } : current),
  });
  const handleVideoFrame = useCallback((nextFrame: VideoFrame) => {
    if (!frameRef.current) setHasVideoFrame(true);
    frameRef.current = nextFrame;
  }, []);

  useEffect(() => {
    if (!IS_TAURI) return;
    const unlistenStatus = listen<ConnectionStatus>(
      "connection-status", (event) => {
        setStatus(event.payload);
        if (event.payload.state === "connected") setPage("session");
        if (event.payload.state === "disconnected" || event.payload.state === "error") { setHasVideoFrame(false); frameRef.current = null; }
      },
    );
    const unlistenFiles = listen<FileEvent>("file-event", (event) => {
      const update = event.payload;
      if (update.kind === "directory") {
        setFilePath(update.path);
        setFileEntries(update.entries);
        setFileError("");
      } else if (update.kind === "directoryCreated") {
        setFileError("");
        void invoke("list_remote_files", { path: parentPath(update.path) });
      } else if (update.kind === "progress") {
        setTransfers((current) => ({ ...current, [update.transferId]: update }));
      } else {
        setFileError(update.message);
        if (update.transferId) {
          setTransfers((current) => {
            const previous = current[update.transferId!];
            return {
              ...current,
              [update.transferId!]: {
                transferId: update.transferId!,
                direction: previous?.direction ?? "transfer",
                transferred: previous?.transferred ?? 0,
                total: previous?.total ?? 0,
                state: "error",
              },
            };
          });
        }
      }
    });
    const unlistenTerminal = listen<TerminalEvent>("terminal-event", (event) => {
      const update = event.payload;
      if (update.kind === "output") {
        setTerminalOutput((current) => (current + update.data).slice(-1_000_000));
      } else if (update.kind === "closed") {
        setTerminalOutput((current) => `${current}\n[terminal closed: ${update.exitCode ?? "signal"}]\n`);
        setTerminalId("");
      } else {
        setTerminalOutput((current) => `${current}\n[error: ${update.message}]\n`);
      }
    });
    const unlistenSystem = listen<SystemSnapshot>("system-info", (event) => {
      setSystemInfo(event.payload);
    });
    return () => {
      void unlistenStatus.then((unlisten) => unlisten());
      void unlistenFiles.then((unlisten) => unlisten());
      void unlistenTerminal.then((unlisten) => unlisten());
      void unlistenSystem.then((unlisten) => unlisten());
      if (moveAnimationFrame.current !== null) {
        cancelAnimationFrame(moveAnimationFrame.current);
      }
    };
  }, []);

  useEffect(() => {
    if (!IS_TAURI) return;
    const legacyController = {
      controlServerUrl: request.controlServerUrl,
      relayAddress: request.relayAddress,
      relayServerName: request.serverName,
      caCertificatePath: request.caCertificatePath,
    };
    void Promise.all([remoteXApi.loadAgentSettings(), remoteXApi.loadServerConfig(legacyController)])
      .then(([loadedAgent, loadedServer]) => {
        setAgentSettings(loadedServer.config.configured ? {
          ...loadedAgent,
          serverUrl: loadedServer.config.controlServerUrl,
          caCertificatePath: loadedServer.config.caCertificatePath,
        } : loadedAgent);
        setServerConfig(loadedServer.config);
        setServerConfigState(loadedServer);
        setShowSetup(loadedServer.needsSetup);
      })
      .catch((error) => setUiError(friendlyError(error)));
    const refresh = () => {
      void invoke<AgentRuntimeStatus>("agent_status")
        .then(setAgentRuntime)
        .catch((error) => setUiError(friendlyError(error)));
    };
    refresh();
    const interval = window.setInterval(refresh, 2_000);
    const unlistenSettings = listen("open-settings", () => {
      setPage("settings"); setSettingsSection("general");
    });
    return () => {
      window.clearInterval(interval);
      void unlistenSettings.then((unlisten) => unlisten());
    };
  }, []); // The initial legacy values are intentionally captured once for migration.

  useEffect(() => {
    if (status.state !== "connected") { setSessionSeconds(0); return; }
    const deviceId = request.deviceId.replace(/\D/g, "");
    if (/^\d{9}$/.test(deviceId)) setRecentDevices((current) => [{ deviceId, connectedAt: Date.now() }, ...current.filter((item) => item.deviceId !== deviceId)].slice(0, 5));
    const interval = window.setInterval(() => setSessionSeconds((seconds) => seconds + 1), 1_000);
    return () => window.clearInterval(interval);
  }, [status.state, request.deviceId]);
  useEffect(() => { localStorage.setItem("remotex-recent-devices", JSON.stringify(recentDevices)); }, [recentDevices]);

  useEffect(() => {
    window.localStorage.setItem("remotex-controller-settings", JSON.stringify({
      controllerName: request.controllerName,
      clipboardEnabled: request.clipboardEnabled,
      fileUploadEnabled: request.fileUploadEnabled,
      fileDownloadEnabled: request.fileDownloadEnabled,
    }));
  }, [request.controllerName, request.clipboardEnabled, request.fileUploadEnabled, request.fileDownloadEnabled]);

  useEffect(() => {
    if (status.state !== "connected") {
      pressedButtons.current.clear();
      pressedKeys.current.clear();
    }
  }, [status.state]);

  async function connect(deviceIdValue = request.deviceId) {
    const deviceId = deviceIdValue.replace(/\D/g, "");
    if (!/^\d{9}$/.test(deviceId)) { setUiError("Enter the 9-digit ID shown on the remote device."); return; }
    setRequest((current) => ({ ...current, deviceId }));
    setUiError("");
    if (!serverConfig.configured) { setShowSetup(true); setUiError("Set up your RemoteX server before connecting."); return; }
    setHasVideoFrame(false); frameRef.current = null;
    setStatus({ state: "connecting", message: "Requesting a secure session…" });
    try { await invoke("connect_remote", { request: {
      ...request, deviceId,
      controlServerUrl: serverConfig.controlServerUrl,
      relayAddress: serverConfig.relayAddress,
      serverName: serverConfig.relayServerName,
      caCertificatePath: serverConfig.caCertificatePath,
    } }); }
    catch (error) { const failure = friendlyFailure(error); setStatus({ state: "failed", message: `${failure.message} ${failure.action}` }); setUiError(failure.title); }
  }

  function updateAgent<K extends keyof AgentSettings>(key: K, value: AgentSettings[K]) {
    setAgentSettings((current) => ({ ...current, [key]: value }));
  }

  async function persistAgent(next: AgentSettings, message = "Changes saved.") {
    setSaving(true); setAgentMessage(""); setUiError(""); setAgentSettings(next);
    try {
      await invoke("save_agent_settings", { settings: next });
      const loaded = await invoke<AgentSettings>("load_agent_settings");
      setAgentSettings(loaded); setAgentMessage(message); return loaded;
    } catch (error) { setUiError(friendlyError(error)); throw error; }
    finally { setSaving(false); }
  }

  async function saveAgent(event: FormEvent) {
    event.preventDefault();
    setAgentMessage("Saving settings…");
    try { await persistAgent(agentSettings, "Settings applied."); } catch { /* handled above */ }
  }

  async function saveNetworkSettings() {
    setSaving(true); setNetworkMessage("Saving and checking the server…"); setUiError("");
    try {
      const check = await remoteXApi.checkServerConnection(serverConfig);
      if (!check.ok) { setNetworkMessage(check.message); return; }
      const saved = await remoteXApi.saveServerConfig(serverConfig);
      setServerConfig(saved);
      setAgentSettings((current) => ({ ...current, serverUrl: saved.controlServerUrl, caCertificatePath: saved.caCertificatePath }));
      setNetworkMessage("Server is ready. Incoming and outgoing connections now use this configuration.");
    } catch (error) { setNetworkMessage(friendlyError(error)); }
    finally { setSaving(false); }
  }

  async function runSetupAgain() {
    try {
      const state = await remoteXApi.resetFirstRun();
      setServerConfigState(state); setShowSetup(true);
    } catch (error) { setUiError(friendlyError(error)); }
  }

  async function toggleRemoteAccess(enabled: boolean) {
    const previous = agentSettings;
    try {
      const saved = await persistAgent({ ...agentSettings, remoteAccessEnabled: enabled }, enabled ? "Remote access enabled." : "Remote access disabled.");
      setAgentRuntime(await invoke<AgentRuntimeStatus>(enabled ? "start_agent" : "stop_agent")); setAgentSettings(saved);
    } catch { setAgentSettings(previous); void invoke<AgentSettings>("load_agent_settings").then(setAgentSettings).catch(() => undefined); }
  }

  async function chooseCertificate(target: "agent" | "controller") {
    try {
      const selected = await open({ multiple: false, directory: false, filters: [{ name: "Certificates", extensions: ["pem", "crt", "cer"] }] });
      if (typeof selected !== "string") return;
      if (target === "agent") updateAgent("caCertificatePath", selected); else setServerConfig((current) => ({ ...current, caCertificatePath: selected }));
    } catch (error) { setUiError(friendlyError(error)); }
  }

  async function addAllowedFolder() {
    try {
      const selected = await open({ multiple: false, directory: true });
      if (typeof selected !== "string") return;
      const folders = parseFolders(agentSettings.fileRoots);
      if (folders.some((folder) => folder.path.toLocaleLowerCase() === selected.toLocaleLowerCase())) return;
      updateAgent("fileRoots", serializeFolders([...folders, { label: selected.split(/[\\/]/).filter(Boolean).at(-1) ?? "Folder", path: selected }]));
    } catch (error) { setUiError(friendlyError(error)); }
  }

  async function copyDeviceId() {
    if (!agentRuntime?.deviceId) return;
    try { await navigator.clipboard.writeText(agentRuntime.deviceId); setCopied(true); window.setTimeout(() => setCopied(false), 1_500); }
    catch (error) { setUiError(friendlyError(error)); }
  }

  async function setAgentRunning(running: boolean) {
    setAgentMessage(running ? "Starting remote access…" : "Disconnecting and stopping remote access…");
    try {
      const next = await invoke<AgentRuntimeStatus>(running ? "start_agent" : "stop_agent");
      setAgentRuntime(next);
      setAgentMessage(running ? "Remote access started." : "Remote access disabled until you start it again.");
    } catch (error) {
      setAgentMessage(String(error));
    }
  }

  async function startTerminal() {
    const id = await invoke<string>("open_terminal");
    setTerminalId(id);
    setTerminalOutput("");
  }

  async function submitTerminal(event: FormEvent) {
    event.preventDefault();
    if (!terminalId || !terminalInput) return;
    await invoke("send_terminal_input", { terminalId, data: `${terminalInput}\n` });
    setTerminalInput("");
  }

  async function listFiles(path: string) {
    setFileError("");
    await invoke("list_remote_files", { path });
  }

  async function createDirectory() {
    const name = newFolderName.trim();
    if (!name || name.includes("/") || name.includes("\\")) return;
    await invoke("create_remote_directory", { path: childPath(filePath, name) });
    setNewFolderName("");
  }

  async function uploadFile() {
    if (!uploadLocalPath || !uploadDestinationPath) return;
    await invoke("upload_remote_file", {
      localPath: uploadLocalPath,
      destinationPath: uploadDestinationPath,
    });
  }

  async function downloadFile() {
    if (!downloadSourcePath || !downloadLocalPath) return;
    await invoke("download_remote_file", {
      sourcePath: downloadSourcePath,
      localPath: downloadLocalPath,
    });
  }

  async function resumeUpload() {
    if (!resumeTransferId || !uploadLocalPath || !uploadDestinationPath) return;
    await invoke("resume_file_upload", {
      transferId: resumeTransferId,
      localPath: uploadLocalPath,
      destinationPath: uploadDestinationPath,
    });
  }

  async function resumeDownload() {
    if (!resumeTransferId || !downloadSourcePath || !downloadLocalPath) return;
    await invoke("resume_file_download", {
      transferId: resumeTransferId,
      sourcePath: downloadSourcePath,
      localPath: downloadLocalPath,
    });
  }

  function enqueueInput(
    command: "send_mouse_input" | "send_keyboard_input",
    inputRequest: MouseInputRequest | KeyboardInputRequest,
  ): Promise<void> {
    const next = inputChain.current
      .then(() => invoke(command, { request: inputRequest }))
      .then(() => undefined)
      .catch((error) => {
        console.error("Remote input failed", error);
      });
    inputChain.current = next;
    return next;
  }

  function sendMouse(mouseRequest: MouseInputRequest): Promise<void> {
    return enqueueInput("send_mouse_input", mouseRequest);
  }

  function sendKeyboard(keyboardRequest: KeyboardInputRequest): Promise<void> {
    return enqueueInput("send_keyboard_input", keyboardRequest);
  }

  async function releaseRemoteButtons() {
    const buttons = Array.from(pressedButtons.current);
    pressedButtons.current.clear();
    await Promise.all(
      buttons.map((button) => sendMouse({ kind: "buttonUp", button })),
    );
  }

  async function releaseRemoteKeys() {
    const keys = Array.from(pressedKeys.current);
    pressedKeys.current.clear();
    await Promise.all(keys.map((key) => sendKeyboard({ kind: "keyUp", key })));
  }

  async function releaseRemoteInputs() {
    await Promise.all([releaseRemoteButtons(), releaseRemoteKeys()]);
  }

  async function disconnect() {
    await releaseRemoteInputs();
    await invoke("disconnect_remote");
  }

  function update(field: keyof ConnectRequest, value: string) {
    setRequest((current) => ({ ...current, [field]: value }));
  }

  function normalizedScreenPoint(clientX: number, clientY: number): UnitPoint | null {
    const screen = screenRef.current;
    const frame = frameRef.current;
    if (!screen || !frame || frame.width === 0 || frame.height === 0) {
      return null;
    }

    const rect = screen.getBoundingClientRect();
    const contentWidth = screen.clientWidth;
    const contentHeight = screen.clientHeight;
    const scale = Math.min(contentWidth / frame.width, contentHeight / frame.height);
    const imageWidth = frame.width * scale;
    const imageHeight = frame.height * scale;
    const imageLeft = rect.left + screen.clientLeft + (contentWidth - imageWidth) / 2;
    const imageTop = rect.top + screen.clientTop + (contentHeight - imageHeight) / 2;
    const x = (clientX - imageLeft) / imageWidth;
    const y = (clientY - imageTop) / imageHeight;
    if (x < 0 || x > 1 || y < 0 || y > 1) {
      return null;
    }
    return { x, y };
  }

  function handlePointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    if (status.state !== "connected") {
      return;
    }
    const point = normalizedScreenPoint(event.clientX, event.clientY);
    if (!point) {
      return;
    }
    pendingMove.current = point;
    if (moveAnimationFrame.current !== null) {
      return;
    }
    moveAnimationFrame.current = requestAnimationFrame(() => {
      moveAnimationFrame.current = null;
      const next = pendingMove.current;
      pendingMove.current = null;
      if (next) {
        void sendMouse({ kind: "move", displayId: null, ...next });
      }
    });
  }

  function handlePointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    if (status.state !== "connected") {
      return;
    }
    const button = mouseButton(event.button);
    const point = normalizedScreenPoint(event.clientX, event.clientY);
    if (!button || !point || pressedButtons.current.has(button)) {
      return;
    }
    event.preventDefault();
    event.currentTarget.focus({ preventScroll: true });
    event.currentTarget.setPointerCapture(event.pointerId);
    pressedButtons.current.add(button);
    void sendMouse({ kind: "buttonDown", button, ...point });
  }

  function handlePointerUp(event: ReactPointerEvent<HTMLDivElement>) {
    const button = mouseButton(event.button);
    if (!button || !pressedButtons.current.delete(button)) {
      return;
    }
    event.preventDefault();
    void sendMouse({ kind: "buttonUp", button });
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  }

  function handlePointerCancel(event: ReactPointerEvent<HTMLDivElement>) {
    event.preventDefault();
    void releaseRemoteButtons();
    if (event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
  }

  function handleWheel(event: ReactWheelEvent<HTMLDivElement>) {
    if (status.state !== "connected" || !frameRef.current) {
      return;
    }
    event.preventDefault();
    const horizontalDelta = -wheelDelta(event.deltaX);
    const verticalDelta = -wheelDelta(event.deltaY);
    if (horizontalDelta !== 0 || verticalDelta !== 0) {
      void sendMouse({ kind: "wheel", horizontalDelta, verticalDelta });
    }
  }

  function handleKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (status.state !== "connected") {
      return;
    }
    const key = REMOTE_KEY_BY_CODE[event.code as keyof typeof REMOTE_KEY_BY_CODE];
    if (!key) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (event.repeat || pressedKeys.current.has(key)) {
      return;
    }
    pressedKeys.current.add(key);
    void sendKeyboard({ kind: "keyDown", key });
  }

  function handleKeyUp(event: ReactKeyboardEvent<HTMLDivElement>) {
    const key = REMOTE_KEY_BY_CODE[event.code as keyof typeof REMOTE_KEY_BY_CODE];
    if (!key || !pressedKeys.current.delete(key)) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    void sendKeyboard({ kind: "keyUp", key });
  }

  const isConnected = status.state === "connected";
  const isConnecting = ["contacting", "authorizing", "connecting"].includes(status.state);
  const isConnectionFailure = ["failed", "error"].includes(status.state);
  const folders = parseFolders(agentSettings.fileRoots);
  const remoteEnabled = agentSettings.remoteAccessEnabled;
  const agentReady = Boolean(agentRuntime?.running && agentRuntime.deviceId);
  const topTitle = page === "session" ? "Active Session" : NAV_ITEMS.find((item) => item.page === page)?.label ?? "RemoteX";
  const duration = `${String(Math.floor(sessionSeconds / 60)).padStart(2, "0")}:${String(sessionSeconds % 60).padStart(2, "0")}`;

  function immediateAgent<K extends keyof AgentSettings>(key: K, value: AgentSettings[K]) {
    const next = { ...agentSettings, [key]: value };
    void persistAgent(next).catch(() => undefined);
  }

  if (showSetup && serverConfigState) {
    return <SetupWizard state={{ ...serverConfigState, config: serverConfig }} onLater={() => setShowSetup(false)} onComplete={(config) => {
      setServerConfig(config); setServerConfigState({ config, needsSetup: false, migrationConflict: false, candidates: [] });
      setAgentSettings((current) => ({ ...current, serverUrl: config.controlServerUrl, caCertificatePath: config.caCertificatePath }));
      setShowSetup(false); setNetworkMessage("Server setup is complete."); setUiError("");
    }} />;
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand"><span className="brand-mark"><img src="/app-icon.png" alt="" /></span><span className="brand-copy"><strong>RemoteX</strong><small>Secure remote access</small></span></div>
        <nav aria-label="Main navigation" className="sidebar-navigation">
          <div className="nav-group nav-primary">
            {PRIMARY_NAV_ITEMS.map((item) => <button key={item.page} type="button" className={page === item.page ? "active" : ""} aria-label={item.label} aria-current={page === item.page ? "page" : undefined} onClick={() => setPage(item.page)}><Icon name={item.icon} /><span>{item.label}</span>{item.page === "devices" && isConnected && <i className="live-dot" />}</button>)}
          </div>
          <div className="nav-group nav-secondary">
            {SECONDARY_NAV_ITEMS.map((item) => <button key={item.page} type="button" className={page === item.page ? "active" : ""} aria-label={item.label} aria-current={page === item.page ? "page" : undefined} onClick={() => setPage(item.page)}><Icon name={item.icon} /><span>{item.label}</span>{item.page === "about" && (["available", "downloading", "ready"] as const).includes(appUpdater.state.phase as "available" | "downloading" | "ready") && <i className="update-dot" title="Software update available" />}</button>)}
          </div>
        </nav>
        <div className="sidebar-status"><span className="sidebar-status-line"><i className={`status-dot ${agentReady ? "success" : remoteEnabled ? "warning" : ""}`} /><span>{agentReady ? "Remote access on" : remoteEnabled ? "Starting…" : "Remote access off"}</span></span><strong>{agentSettings.deviceName || "This Windows PC"}</strong>{agentRuntime?.deviceId && <small>{formatDeviceId(agentRuntime.deviceId)}</small>}</div>
      </aside>

      <main className={`main-area ${page === "home" || page === "session" ? "without-topbar" : ""}`}>
        {page !== "home" && page !== "session" && <header className="topbar"><div><h1>{topTitle}</h1><p>{page === "devices" ? "Manage this PC and recent connections." : page === "files" ? "Browse transfers and remote tools." : page === "settings" ? "Manage RemoteX preferences and behavior." : "RemoteX product and update information."}</p></div>{isConnected && <button type="button" className="session-chip" onClick={() => setPage("session")}><i className="live-dot" /><span>{formatDeviceId(request.deviceId)} · {duration}</span><Icon name="chevron" size={14} /></button>}</header>}

        <div className={`page-content page-${page}`}>
          {page === "home" && <HomePage
            deviceId={request.deviceId}
            accessPassword={request.unattendedSecret}
            recentDevices={recentDevices}
            localDeviceName={agentSettings.deviceName}
            localDeviceId={agentRuntime?.deviceId ?? null}
            remoteAccessEnabled={remoteEnabled}
            localDeviceReady={agentReady}
            saving={saving}
            copied={copied}
            agentMessage={agentMessage}
            connectionState={status.state}
            connectionMessage={status.message}
            connectionError={uiError}
            connecting={isConnecting}
            connected={isConnected}
            connectionFailed={isConnectionFailure}
            onDeviceIdChange={(deviceId) => { update("deviceId", deviceId); setUiError(""); }}
            onAccessPasswordChange={(password) => update("unattendedSecret", password)}
            onConnect={connect}
            onViewAllDevices={() => setPage("devices")}
            onOpenSettings={() => { setPage("settings"); setSettingsSection("security"); }}
            onToggleRemoteAccess={(enabled) => void toggleRemoteAccess(enabled)}
            onCopyDeviceId={() => void copyDeviceId()}
          />}

          {page === "devices" && <div className="standard-page devices-page">
            <section className="panel local-device-panel"><span className="local-device-label"><Icon name="monitor" size={14} />This Device</span><span className="device-avatar"><Icon name="monitor" size={25} /></span><div className="grow"><strong>{agentSettings.deviceName || "This Windows PC"}</strong><small>{agentReady ? "Online and ready for secure access" : remoteEnabled ? "Remote access is starting" : "Remote access is disabled"}</small></div><StatusPill tone={agentReady ? "success" : remoteEnabled ? "warning" : "neutral"}>{agentReady ? "Ready" : remoteEnabled ? "Starting" : "Offline"}</StatusPill><div className="local-device-meta"><span><small>Device ID</small><strong>{agentRuntime?.deviceId ? formatDeviceId(agentRuntime.deviceId) : "Available after Remote Access starts"}</strong></span><button type="button" className="secondary-button" onClick={() => { setPage("settings"); setSettingsSection("remote"); }}><Icon name="settings" size={15} />Device settings</button></div><span className="device-hero-art" aria-hidden="true"><Icon name="monitor" size={88} /></span></section>
            {isConnected && <section className="active-session-panel"><i className="live-dot" /><div><strong>Active session · {formatDeviceId(request.deviceId)}</strong><small>Connected for {duration}</small></div><button type="button" className="danger-quiet" onClick={() => void disconnect()}><Icon name="disconnect" size={15} />Disconnect</button></section>}
            <section className="panel recent-devices-panel"><div className="section-heading"><div><h3>Recent devices</h3><p>Created from successful connections on this PC.</p></div><Icon name="refresh" size={17} /></div>{recentDevices.length ? <div className="device-list">{recentDevices.map((device, index) => <div className="device-list-row" key={device.deviceId}><span className={`device-avatar small tone-${index % 4}`}><Icon name="monitor" size={16} /></span><div className="grow"><strong>{formatDeviceId(device.deviceId)}</strong><small>Last connected {new Date(device.connectedAt).toLocaleString()}</small></div><span className="device-last-used">{new Date(device.connectedAt).toLocaleDateString()}</span><button type="button" className="secondary-button" onClick={() => { update("deviceId", device.deviceId); setPage("home"); }}>Connect<Icon name="chevron" size={13} /></button></div>)}</div> : <div className="empty-state compact"><span className="empty-state-symbol"><Icon name="devices" size={28} /></span><h3>No recent devices</h3><p>Devices appear here after your first successful connection.</p><button type="button" className="secondary-button" onClick={() => setPage("home")}>Connect a device</button></div>}</section>
          </div>}

          {page === "session" && <div className="session-page">
            <div className="session-toolbar"><div><i className="live-dot" /><span><strong>{formatDeviceId(request.deviceId)}</strong><small>Secure session · {duration}</small></span></div><div className="session-actions"><button type="button" className="secondary-button" onClick={() => setPage("files")}><Icon name="files" size={15} />Files</button><button type="button" className="danger-quiet" onClick={() => void disconnect()}><Icon name="disconnect" size={15} />Disconnect</button></div></div>
            <div ref={screenRef} className={`screen ${isConnected && hasVideoFrame ? "interactive" : ""}`} aria-label="Remote desktop" role="application" tabIndex={isConnected && hasVideoFrame ? 0 : -1} onContextMenu={(event) => event.preventDefault()} onPointerMove={handlePointerMove} onPointerDown={handlePointerDown} onPointerUp={handlePointerUp} onPointerCancel={handlePointerCancel} onWheel={handleWheel} onKeyDown={handleKeyDown} onKeyUp={handleKeyUp} onBlur={() => void releaseRemoteInputs()}>
              <RemoteVideoRenderer connected={isConnected} statusMessage={status.message} onFrame={handleVideoFrame} />
            </div>
          </div>}

          {page === "files" && <div className="standard-page files-page">
            <div className="files-status-strip"><div><span className="feature-icon tone-blue"><Icon name="monitor" size={17} /></span><span><strong>{isConnected ? formatDeviceId(request.deviceId) : "No device connected"}</strong><small>{isConnected ? `Secure session · ${duration}` : "Connect a device to unlock file tools"}</small></span></div><StatusPill tone={isConnected ? "success" : "neutral"}>{isConnected ? "Session active" : "No active session"}</StatusPill></div>
            {!isConnected ? <><section className="quick-tools-panel panel"><div className="section-heading"><div><h3>Quick tools</h3><p>Available during an authorized session.</p></div></div><div className="quick-tool-grid"><div><span className="feature-icon tone-blue"><Icon name="folder" /></span><strong>File browser</strong><small>Browse remote files</small></div><div><span className="feature-icon tone-green"><Icon name="upload" /></span><strong>Upload</strong><small>Send files securely</small></div><div><span className="feature-icon tone-amber"><Icon name="download" /></span><strong>Download</strong><small>Save files locally</small></div><div><span className="feature-icon tone-purple"><Icon name="terminal" /></span><strong>Terminal</strong><small>Open a remote PTY</small></div></div></section><section className="panel empty-state files-empty-state"><span className="empty-state-symbol tone-blue"><Icon name="files" size={30} /></span><h3>Connect to use remote tools</h3><p>File browsing, terminal access, and system details are available during an authorized session.</p><button type="button" onClick={() => setPage("home")}>Connect a device<Icon name="chevron" size={14} /></button></section></> : <>
              <section className="panel file-browser"><div className="section-heading"><div><h3>Remote files</h3><p>{filePath}</p></div><div className="button-group"><button type="button" className="secondary-button" disabled={filePath === "/"} onClick={() => void listFiles(parentPath(filePath))}>Up</button><button type="button" className="icon-button" aria-label="Refresh files" onClick={() => void listFiles(filePath)}><Icon name="refresh" size={15} /></button></div></div>{fileError && <p className="inline-message"><Icon name="warning" size={14} />{fileError}</p>}<div className="file-table-wrap"><table><thead><tr><th>Name</th><th>Size</th><th>Modified</th></tr></thead><tbody>{fileEntries.map((entry) => <tr key={entry.path}><td><button type="button" className="file-link" onClick={() => entry.entryType === "directory" ? void listFiles(entry.path) : setDownloadSourcePath(entry.path)}><Icon name={entry.entryType === "directory" ? "folder" : "files"} size={15} />{entry.name}</button></td><td>{entry.entryType === "file" ? formatBytes(entry.size) : "—"}</td><td>{entry.modifiedMs ? new Date(entry.modifiedMs).toLocaleString() : "—"}</td></tr>)}{!fileEntries.length && <tr><td colSpan={3} className="table-empty">Refresh to load this directory.</td></tr>}</tbody></table></div></section>
              <div className="tool-grid"><section className="panel tool-card tone-blue"><span className="feature-icon"><Icon name="plus" /></span><h3>New folder</h3><input value={newFolderName} onChange={(event) => setNewFolderName(event.target.value)} placeholder="Folder name" /><button type="button" disabled={!request.fileUploadEnabled} onClick={() => void createDirectory()}>Create folder</button></section><section className="panel tool-card tone-green"><span className="feature-icon"><Icon name="upload" /></span><h3>Upload</h3><input value={uploadLocalPath} onChange={(event) => setUploadLocalPath(event.target.value)} placeholder="Local source path" /><input value={uploadDestinationPath} onChange={(event) => setUploadDestinationPath(event.target.value)} placeholder="Remote destination path" /><button type="button" disabled={!request.fileUploadEnabled} onClick={() => void uploadFile()}>Upload</button></section><section className="panel tool-card tone-amber"><span className="feature-icon"><Icon name="download" /></span><h3>Download</h3><input value={downloadSourcePath} onChange={(event) => setDownloadSourcePath(event.target.value)} placeholder="Remote source path" /><input value={downloadLocalPath} onChange={(event) => setDownloadLocalPath(event.target.value)} placeholder="Local destination path" /><button type="button" disabled={!request.fileDownloadEnabled} onClick={() => void downloadFile()}>Download</button></section></div>
              {Object.keys(transfers).length > 0 && <section className="panel"><h3>Transfers</h3><div className="transfers">{Object.values(transfers).map((transfer) => <div className="transfer" key={transfer.transferId}><div><strong>{transfer.direction}</strong><span>{transfer.state} · {formatBytes(transfer.transferred)}</span></div><progress max={100} value={transfer.total ? transfer.transferred / transfer.total * 100 : 0} /><button type="button" className="secondary-button" onClick={() => void invoke("cancel_file_transfer", { transferId: transfer.transferId })}>Cancel</button></div>)}</div><div className="resume-row"><input value={resumeTransferId} onChange={(event) => setResumeTransferId(event.target.value)} placeholder="Interrupted transfer ID" /><button type="button" className="secondary-button" onClick={() => void resumeUpload()}>Resume upload</button><button type="button" className="secondary-button" onClick={() => void resumeDownload()}>Resume download</button></div></section>}
              <section className="panel server-tools"><div className="section-heading"><div><h3>Linux terminal</h3><p>Open an authorized remote PTY.</p></div><div className="button-group"><button type="button" disabled={Boolean(terminalId)} onClick={() => void startTerminal()}>Open terminal</button><button type="button" className="secondary-button" disabled={!terminalId} onClick={() => { void invoke("close_terminal", { terminalId }); setTerminalId(""); }}>Close</button></div></div><pre className="terminal">{terminalOutput ? renderAnsi(terminalOutput) : "Terminal output will appear here."}</pre><form className="terminal-input" onSubmit={submitTerminal}><input value={terminalInput} onChange={(event) => setTerminalInput(event.target.value)} disabled={!terminalId} placeholder="Command or terminal input" /><button type="submit" disabled={!terminalId || !terminalInput}>Send</button><button type="button" className="secondary-button" disabled={!terminalId} onClick={() => void invoke("send_terminal_input", { terminalId, data: "\u0003" })}>Ctrl+C</button></form><div className="system-summary"><div className="section-heading"><div><h3>{systemInfo?.hostname ?? "System information"}</h3><p>OS, CPU, memory, storage, network, and GPU summary.</p></div><button type="button" className="secondary-button" onClick={() => void invoke("request_system_info")}>Refresh</button></div>{systemInfo && <div className="system-grid"><div><span>OS</span><strong>{systemInfo.operatingSystem}</strong></div><div><span>CPU</span><strong>{systemInfo.cpuModel}</strong></div><div><span>Memory</span><strong>{formatBytes(systemInfo.usedMemoryBytes)} / {formatBytes(systemInfo.totalMemoryBytes)}</strong></div><div><span>GPU</span><strong>{systemInfo.gpus.map((gpu) => gpu.name).join(", ") || "Not detected"}</strong></div></div>}</div></section>
            </>}
          </div>}

          {page === "settings" && <div className="settings-layout" ref={settingsRef as React.RefObject<HTMLDivElement>}>
            <nav className="settings-nav panel" aria-label="Settings categories">{SETTINGS_SECTIONS.map((section) => <button type="button" key={section.id} className={settingsSection === section.id ? "active" : ""} onClick={() => setSettingsSection(section.id)}><span className="settings-nav-label"><Icon name={section.icon} size={17} /><span>{section.label}</span></span><Icon name="chevron" size={13} /></button>)}</nav>
            <form className="settings-pane panel" onSubmit={saveAgent}><div className="settings-pane-header"><span className="settings-pane-symbol"><Icon name={SETTINGS_SECTIONS.find((item) => item.id === settingsSection)?.icon ?? "settings"} size={22} /></span><div className="grow"><h2>{SETTINGS_SECTIONS.find((item) => item.id === settingsSection)?.label}</h2><p>Settings marked with a switch are saved immediately.</p></div><span className="save-state">{saving ? <><span className="spinner" />Saving…</> : agentMessage || uiError}</span></div><div className="settings-groups">
              {settingsSection === "general" && <section className="settings-group"><label className="field-label">Device name<input value={agentSettings.deviceName} onChange={(event) => updateAgent("deviceName", event.target.value)} maxLength={128} /><small>Shown to people connecting to this PC.</small></label><label className="field-label">Connection name<input value={request.controllerName} onChange={(event) => update("controllerName", event.target.value)} maxLength={128} /><small>Shown on remote devices when you connect.</small></label><label className="setting-row language-setting"><span><strong>{t("Interface language", "界面语言")}</strong><small>{t("Choose the language used throughout RemoteX.", "选择 RemoteX 全部界面使用的语言。")}</small></span><select aria-label={t("Interface language", "界面语言")} value={language} onChange={(event) => setLanguage(event.target.value as AppLanguage)}><option value="en">English</option><option value="zh-CN">简体中文</option></select></label><div className="setting-row"><span><strong>Start with Windows</strong><small>Launch RemoteX after you sign in.</small></span><Toggle label="Start with Windows" checked={agentSettings.startWithWindows} onChange={(value) => immediateAgent("startWithWindows", value)} /></div><AppUpdaterSettings updater={appUpdater} /></section>}
              {settingsSection === "appearance" && <section className="settings-group appearance-settings"><div className="setting-copy"><strong>Color mode</strong><small>Choose how RemoteX and its Windows title bar appear.</small></div><div className="appearance-options" role="radiogroup" aria-label="Color mode">{APPEARANCE_OPTIONS.map((option) => <button key={option.id} type="button" role="radio" aria-checked={appearance === option.id} className={appearance === option.id ? "selected" : ""} onClick={() => setAppearance(option.id)}><span className={`appearance-preview ${option.id}`}><Icon name={option.icon} size={18} /></span><span><strong>{option.label}</strong><small>{option.detail}</small></span>{appearance === option.id && <Icon name="check" size={15} />}</button>)}</div><p className="settings-note"><Icon name="sun" size={14} />Changes apply immediately and are saved on this PC.</p></section>}
              {settingsSection === "remote" && <section className="settings-group"><div className="setting-row emphasized"><span><strong>Allow remote access</strong><small>Makes this computer available through your configured server.</small></span><Toggle label="Allow remote access" checked={remoteEnabled} disabled={saving} onChange={(value) => void toggleRemoteAccess(value)} /></div></section>}
              {settingsSection === "permissions" && <><section className="settings-group">{([["allowInput","Keyboard and mouse","Allow remote input"],["allowClipboard","Plain-text clipboard","Allow clipboard synchronization"],["allowFileUpload","File upload","Allow files to be sent to this PC"],["allowFileDownload","File download","Allow files to be downloaded from this PC"]] as const).map(([key,title,detail]) => <div className="setting-row" key={key}><span><strong>{title}</strong><small>{detail}</small></span><Toggle label={title} checked={agentSettings[key]} onChange={(value) => immediateAgent(key, value)} /></div>)}</section><section className="settings-group"><div className="section-heading"><div><h3>Allowed folders</h3><p>Remote file access stays inside these locations.</p></div><button type="button" className="secondary-button" onClick={() => void addAllowedFolder()}><Icon name="plus" size={15} />Add folder</button></div>{folders.length ? <div className="folder-list">{folders.map((folder, index) => <div className="folder-row" key={folder.path}><span className="folder-icon"><Icon name="folder" size={16} /></span><div className="grow"><input aria-label={`Folder ${index + 1} label`} value={folder.label} onChange={(event) => updateAgent("fileRoots", serializeFolders(folders.map((item, itemIndex) => itemIndex === index ? { ...item, label: event.target.value } : item)))} /><small>{folder.path}</small></div><button type="button" className="icon-button danger" aria-label={`Remove ${folder.label}`} onClick={() => updateAgent("fileRoots", serializeFolders(folders.filter((_, itemIndex) => itemIndex !== index)))}><Icon name="trash" size={15} /></button></div>)}</div> : <div className="empty-inline"><Icon name="folder" /><span><strong>No folders allowed</strong><small>File permissions remain unavailable until you add one.</small></span></div>}</section></>}
              {settingsSection === "video" && <section className="settings-group"><label className="field-label">Video quality<select value={agentSettings.videoQuality} onChange={(event) => immediateAgent("videoQuality", event.target.value as AgentSettings["videoQuality"])}><option value="low">Low · up to 10 FPS</option><option value="balanced">Balanced · up to 20 FPS</option><option value="high">High · up to 30 FPS</option></select><small>Balanced is recommended for most networks.</small></label></section>}
              {settingsSection === "network" && <><section className="settings-group"><label className="field-label">RemoteX server<input value={serverConfig.controlServerUrl} onChange={(event) => setServerConfig((current) => ({ ...current, controlServerUrl: event.target.value, configured: false }))} placeholder="https://remote.example.com" /><small>Used by this computer and every outgoing connection.</small></label><label className="field-label">Relay CA certificate<div className="path-picker"><input value={serverConfig.caCertificatePath} onChange={(event) => setServerConfig((current) => ({ ...current, caCertificatePath: event.target.value, configured: false }))} placeholder="Select a PEM, CRT, or CER file" /><button type="button" className="secondary-button" onClick={() => void chooseCertificate("controller")}>Choose…</button></div></label><details className="advanced-details"><summary>Advanced endpoints</summary><div className="options-grid"><label>Relay address<input value={serverConfig.relayAddress} onChange={(event) => setServerConfig((current) => ({ ...current, relayAddress: event.target.value, configured: false }))} placeholder="Optional managed-session fallback" /></label><label>TLS server name<input value={serverConfig.relayServerName} onChange={(event) => setServerConfig((current) => ({ ...current, relayServerName: event.target.value, configured: false }))} /></label></div></details><div className="network-actions"><button type="button" disabled={saving} onClick={() => void saveNetworkSettings()}>Save and check</button><button type="button" className="secondary-button" onClick={() => void runSetupAgain()}>Run setup again</button></div>{networkMessage && <p className="inline-message success" role="status">{networkMessage}</p>}</section><section className="settings-group"><div className="setting-row"><span><strong>Clipboard for outgoing sessions</strong><small>Request clipboard permission when connecting.</small></span><Toggle label="Request clipboard access" checked={request.clipboardEnabled} onChange={(value) => setRequest((current) => ({ ...current, clipboardEnabled: value }))} /></div><div className="setting-row"><span><strong>File upload for outgoing sessions</strong></span><Toggle label="Request file upload" checked={request.fileUploadEnabled} onChange={(value) => setRequest((current) => ({ ...current, fileUploadEnabled: value }))} /></div><div className="setting-row"><span><strong>File download for outgoing sessions</strong></span><Toggle label="Request file download" checked={request.fileDownloadEnabled} onChange={(value) => setRequest((current) => ({ ...current, fileDownloadEnabled: value }))} /></div></section></>}
              {settingsSection === "security" && <section className="settings-group"><div className="setting-row"><span><strong>Unattended access</strong><small>Requires an explicit secret of at least 12 characters.</small></span><Toggle label="Unattended access" checked={agentSettings.unattendedAccess} onChange={(value) => updateAgent("unattendedAccess", value)} /></div>{agentSettings.unattendedAccess && <label className="field-label">Unattended secret<input type="password" value={agentSettings.unattendedSecret} onChange={(event) => updateAgent("unattendedSecret", event.target.value)} minLength={12} maxLength={128} placeholder={agentSettings.secretConfigured ? "Leave blank to keep the protected secret" : "At least 12 characters"} /></label>}</section>}
              {settingsSection === "advanced" && <><section className="settings-group"><label className="field-label">Agent CA certificate<div className="path-picker"><input value={agentSettings.caCertificatePath} onChange={(event) => updateAgent("caCertificatePath", event.target.value)} placeholder="Select a PEM, CRT, or CER file" /><button type="button" className="secondary-button" onClick={() => void chooseCertificate("agent")}>Choose…</button></div></label><label className="field-label">Controller CA certificate<div className="path-picker"><input value={request.caCertificatePath} onChange={(event) => update("caCertificatePath", event.target.value)} /><button type="button" className="secondary-button" onClick={() => void chooseCertificate("controller")}>Choose…</button></div></label></section><details className="advanced-details"><summary>Manual relay session fallback</summary><div className="options-grid"><label>Relay address<input value={request.relayAddress} onChange={(event) => update("relayAddress", event.target.value)} /></label><label>TLS server name<input value={request.serverName} onChange={(event) => update("serverName", event.target.value)} /></label><label>Session ID<input value={request.sessionId} onChange={(event) => update("sessionId", event.target.value)} /></label><label>One-time token<input type="password" value={request.tokenHex} onChange={(event) => update("tokenHex", event.target.value)} /></label><label>End-to-end key<input type="password" value={request.endToEndKeyHex} onChange={(event) => update("endToEndKeyHex", event.target.value)} /></label></div></details></>}
              {(["general","permissions","security","advanced"] as SettingsSection[]).includes(settingsSection) && <button type="submit" disabled={saving}>Apply</button>}
            </div></form>
          </div>}

          {page === "about" && <div className="about-page"><div className="about-stars" aria-hidden="true"><i /><i /><i /><i /><i /></div><span className="about-mark"><img src="/app-icon.png" alt="RemoteX icon" /></span><h2>RemoteX</h2><p className="version">Version {appUpdater.appVersion}</p><p>A private remote desktop for your own Windows PCs and Linux servers, designed around explicit permissions and end-to-end encryption.</p><AppUpdatePanel updater={appUpdater} /><div className="about-grid"><div className="tone-blue"><span className="about-feature-icon"><Icon name="shield" /></span><span><strong>Private</strong><small>Self-hosted coordination keeps session data under your control.</small></span></div><div className="tone-green"><span className="about-feature-icon"><Icon name="wifi" /></span><span><strong>Responsive</strong><small>Adaptive remote video balances quality and latency.</small></span></div><div className="tone-purple"><span className="about-feature-icon"><Icon name="server" /></span><span><strong>Capable</strong><small>Desktop, file, clipboard, and server tools in one app.</small></span></div><div className="tone-amber"><span className="about-feature-icon"><Icon name="power" /></span><span><strong>Reliable</strong><small>Signed updates and explicit authorization protect every connection.</small></span></div></div><small className="copyright">© 2026 RemoteX</small></div>}
        </div>
      </main>
      <DiagnosticsOverlay connectionState={status.state} />
    </div>
  );

}

type RemoteXRootElement = HTMLElement & { __remoteXRoot?: Root };
const rootElement = document.getElementById("root") as RemoteXRootElement;
const root = rootElement.__remoteXRoot ?? createRoot(rootElement);
rootElement.__remoteXRoot = root;
root.render(<I18nProvider><App /></I18nProvider>);
