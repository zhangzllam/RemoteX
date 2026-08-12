import {
  FormEvent,
  KeyboardEvent as ReactKeyboardEvent,
  PointerEvent as ReactPointerEvent,
  type ReactNode,
  WheelEvent as ReactWheelEvent,
  useEffect,
  useRef,
  useState,
} from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./style.css";

type VideoFrame = {
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
  endToEndLatencyMs: number;
  codec: string;
  keyFrame: boolean;
  mimeType: string;
  data: string;
};

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

const initialRequest: ConnectRequest = {
  controlServerUrl: "http://127.0.0.1:8080",
  deviceId: "",
  controllerName: "RemoteX Desktop",
  unattendedSecret: "",
  relayAddress: "127.0.0.1:7443",
  serverName: "localhost",
  caCertificatePath: "",
  sessionId: "",
  tokenHex: "",
  endToEndKeyHex: "",
  clipboardEnabled: false,
  fileUploadEnabled: false,
  fileDownloadEnabled: false,
};

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
  const [request, setRequest] = useState(initialRequest);
  const [status, setStatus] = useState<ConnectionStatus>({
    state: "disconnected",
    message: "Not connected",
  });
  const [frame, setFrame] = useState<VideoFrame | null>(null);
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
  const screenRef = useRef<HTMLDivElement>(null);
  const videoCanvasRef = useRef<HTMLCanvasElement>(null);
  const pressedButtons = useRef(new Set<RemoteMouseButton>());
  const pressedKeys = useRef(new Set<RemoteKeyCode>());
  const pendingMove = useRef<UnitPoint | null>(null);
  const moveAnimationFrame = useRef<number | null>(null);
  const inputChain = useRef<Promise<void>>(Promise.resolve());

  useEffect(() => {
    const unlistenFrame = listen<VideoFrame>("video-frame", (event) => {
      setFrame(event.payload);
    });
    const unlistenStatus = listen<ConnectionStatus>(
      "connection-status",
      (event) => setStatus(event.payload),
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
      void unlistenFrame.then((unlisten) => unlisten());
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
    if (!frame || frame.mimeType !== "application/x-remotex-rgba") return;
    const canvas = videoCanvasRef.current;
    const context = canvas?.getContext("2d");
    if (!canvas || !context) return;
    const binary = window.atob(frame.data);
    const rgba = new Uint8ClampedArray(binary.length);
    for (let index = 0; index < binary.length; index += 1) {
      rgba[index] = binary.charCodeAt(index);
    }
    if (rgba.length !== frame.width * frame.height * 4) return;
    canvas.width = frame.width;
    canvas.height = frame.height;
    context.putImageData(new ImageData(rgba, frame.width, frame.height), 0, 0);
  }, [frame]);

  useEffect(() => {
    if (status.state !== "connected") {
      pressedButtons.current.clear();
      pressedKeys.current.clear();
    }
  }, [status.state]);

  async function connect(event: FormEvent) {
    event.preventDefault();
    setFrame(null);
    await invoke("connect_remote", { request });
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
    if (status.state !== "connected" || !frame) {
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

  const connected = status.state === "connected" || status.state === "connecting";
  const interactive = status.state === "connected" && frame !== null;
  const rawFrame = frame?.mimeType === "application/x-remotex-rgba";
  const imageUrl = frame && !rawFrame ? `data:${frame.mimeType};base64,${frame.data}` : undefined;

  return (
    <main>
      <header>
        <div>
          <p className="eyebrow">SELF-HOSTED REMOTE DESKTOP</p>
          <h1>RemoteX</h1>
        </div>
        <span className={`status ${status.state}`}>{status.message}</span>
      </header>

      <section className="workspace">
        <form onSubmit={connect}>
          <label>
            Control server URL
            <input value={request.controlServerUrl} onChange={(e) => update("controlServerUrl", e.target.value)} placeholder="https://control.example.com" />
          </label>
          <label>
            Remote device ID
            <input value={request.deviceId} onChange={(e) => update("deviceId", e.target.value)} inputMode="numeric" pattern="[0-9]{9}" required={Boolean(request.controlServerUrl.trim())} />
          </label>
          <label>
            Controller name
            <input value={request.controllerName} onChange={(e) => update("controllerName", e.target.value)} required={Boolean(request.controlServerUrl.trim())} />
          </label>
          <label>
            Unattended access secret (optional)
            <input type="password" value={request.unattendedSecret} onChange={(e) => update("unattendedSecret", e.target.value)} minLength={12} maxLength={128} />
          </label>
          <label>
            CA certificate path
            <input value={request.caCertificatePath} onChange={(e) => update("caCertificatePath", e.target.value)} required />
          </label>
          <details>
            <summary>Manual session fallback</summary>
            <label>
              Relay address
              <input value={request.relayAddress} onChange={(e) => update("relayAddress", e.target.value)} required={!request.controlServerUrl.trim()} />
            </label>
            <label>
              TLS server name
              <input value={request.serverName} onChange={(e) => update("serverName", e.target.value)} required={!request.controlServerUrl.trim()} />
            </label>
            <label>
              Session ID
              <input value={request.sessionId} onChange={(e) => update("sessionId", e.target.value)} required={!request.controlServerUrl.trim()} />
            </label>
            <label>
              One-time controller token
              <input type="password" value={request.tokenHex} onChange={(e) => update("tokenHex", e.target.value)} minLength={64} maxLength={64} required={!request.controlServerUrl.trim()} />
            </label>
            <label>
              End-to-end session key
              <input type="password" value={request.endToEndKeyHex} onChange={(e) => update("endToEndKeyHex", e.target.value)} minLength={64} maxLength={64} required={!request.controlServerUrl.trim()} />
            </label>
          </details>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={request.clipboardEnabled}
              onChange={(event) =>
                setRequest((current) => ({
                  ...current,
                  clipboardEnabled: event.target.checked,
                }))
              }
            />
            <span>Sync plain-text clipboard</span>
          </label>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={request.fileUploadEnabled}
              onChange={(event) =>
                setRequest((current) => ({
                  ...current,
                  fileUploadEnabled: event.target.checked,
                }))
              }
            />
            <span>Allow file upload</span>
          </label>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={request.fileDownloadEnabled}
              onChange={(event) =>
                setRequest((current) => ({
                  ...current,
                  fileDownloadEnabled: event.target.checked,
                }))
              }
            />
            <span>Allow file download</span>
          </label>
          <div className="actions">
            <button type="submit" disabled={connected}>Connect</button>
            <button type="button" className="secondary" onClick={disconnect} disabled={!connected}>Disconnect</button>
          </div>
        </form>

        <div className="content-column">
        <div
          ref={screenRef}
          className={`screen ${interactive ? "interactive" : ""}`}
          aria-live="polite"
          aria-label="Remote desktop"
          role="application"
          tabIndex={interactive ? 0 : -1}
          onContextMenu={(event) => event.preventDefault()}
          onPointerMove={handlePointerMove}
          onPointerDown={handlePointerDown}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerCancel}
          onWheel={handleWheel}
          onKeyDown={handleKeyDown}
          onKeyUp={handleKeyUp}
          onBlur={() => void releaseRemoteInputs()}
        >
          {rawFrame ? (
            <canvas ref={videoCanvasRef} aria-label="Decoded remote desktop" />
          ) : imageUrl ? (
            <img src={imageUrl} alt="Remote desktop" draggable={false} />
          ) : (
            <div className="empty">
              <span>Remote display</span>
              <small>Adaptive H.264 stream · JPEG fallback</small>
            </div>
          )}
          {frame && (
            <div className="telemetry">
              {frame.width}×{frame.height} · {frame.codec} · {frame.framesPerSecond} FPS · {Math.round(frame.bitrateBps / 1000)} kbps · {frame.endToEndLatencyMs} ms
            </div>
          )}
        </div>

        <section className="files-panel">
          <div className="files-header">
            <div>
              <p className="eyebrow">REMOTE FILES</p>
              <h2>{filePath}</h2>
            </div>
            <div className="inline-actions">
              <button type="button" className="secondary" disabled={status.state !== "connected" || filePath === "/"} onClick={() => void listFiles(parentPath(filePath))}>Up</button>
              <button type="button" className="secondary" disabled={status.state !== "connected"} onClick={() => void listFiles(filePath)}>Refresh</button>
            </div>
          </div>
          {fileError && <p className="file-error">{fileError}</p>}
          <div className="file-table-wrap">
            <table>
              <thead><tr><th>Name</th><th>Size</th><th>Modified</th><th>Type</th></tr></thead>
              <tbody>
                {fileEntries.map((entry) => (
                  <tr key={entry.path} onDoubleClick={() => {
                    if (entry.entryType === "directory") void listFiles(entry.path);
                    else setDownloadSourcePath(entry.path);
                  }}>
                    <td><button type="button" className="file-link" onClick={() => entry.entryType === "directory" ? void listFiles(entry.path) : setDownloadSourcePath(entry.path)}>{entry.name}</button></td>
                    <td>{entry.entryType === "file" ? formatBytes(entry.size) : "—"}</td>
                    <td>{entry.modifiedMs ? new Date(entry.modifiedMs).toLocaleString() : "—"}</td>
                    <td>{entry.entryType}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>

          <div className="file-tools">
            <div>
              <h3>New folder</h3>
              <input value={newFolderName} onChange={(event) => setNewFolderName(event.target.value)} placeholder="Folder name" />
              <button type="button" disabled={status.state !== "connected" || !request.fileUploadEnabled} onClick={() => void createDirectory()}>Create</button>
            </div>
            <div>
              <h3>Upload</h3>
              <input value={uploadLocalPath} onChange={(event) => setUploadLocalPath(event.target.value)} placeholder="Local source file" />
              <input value={uploadDestinationPath} onChange={(event) => setUploadDestinationPath(event.target.value)} placeholder="Remote destination, e.g. /Data/file.zip" />
              <button type="button" disabled={status.state !== "connected" || !request.fileUploadEnabled} onClick={() => void uploadFile()}>Upload</button>
            </div>
            <div>
              <h3>Download</h3>
              <input value={downloadSourcePath} onChange={(event) => setDownloadSourcePath(event.target.value)} placeholder="Remote source file" />
              <input value={downloadLocalPath} onChange={(event) => setDownloadLocalPath(event.target.value)} placeholder="Local destination file" />
              <button type="button" disabled={status.state !== "connected" || !request.fileDownloadEnabled} onClick={() => void downloadFile()}>Download</button>
            </div>
          </div>

          <div className="resume-tools">
            <input value={resumeTransferId} onChange={(event) => setResumeTransferId(event.target.value)} placeholder="Interrupted transfer ID" />
            <button type="button" className="secondary" disabled={status.state !== "connected" || !request.fileUploadEnabled} onClick={() => void resumeUpload()}>Resume upload</button>
            <button type="button" className="secondary" disabled={status.state !== "connected" || !request.fileDownloadEnabled} onClick={() => void resumeDownload()}>Resume download</button>
          </div>

          <div className="transfers">
            {Object.values(transfers).map((transfer) => {
              const percent = transfer.total > 0 ? Math.min(100, transfer.transferred / transfer.total * 100) : 0;
              return <div className="transfer" key={transfer.transferId}>
                <div><strong>{transfer.direction}</strong><span>{transfer.state} · {formatBytes(transfer.transferred)}{transfer.total > 0 ? ` / ${formatBytes(transfer.total)}` : ""}</span></div>
                <progress max={100} value={percent} />
                {!(["completed", "cancelled", "error"].includes(transfer.state)) && <button type="button" className="secondary" onClick={() => void invoke("cancel_file_transfer", { transferId: transfer.transferId })}>Cancel</button>}
              </div>;
            })}
          </div>
        </section>

        <section className="server-panel">
          <div className="files-header">
            <div><p className="eyebrow">LINUX SERVER</p><h2>Terminal</h2></div>
            <div className="inline-actions">
              <button type="button" disabled={status.state !== "connected" || Boolean(terminalId)} onClick={() => void startTerminal()}>Open terminal</button>
              <button type="button" className="secondary" disabled={!terminalId} onClick={() => void invoke("resize_terminal", { terminalId, columns: 160, rows: 40 })}>Resize 160×40</button>
              <button type="button" className="secondary" disabled={!terminalId} onClick={() => {
                void invoke("close_terminal", { terminalId });
                setTerminalId("");
              }}>Close</button>
            </div>
          </div>
          <pre className="terminal" aria-live="polite">{terminalOutput ? renderAnsi(terminalOutput) : "Connect to an authorized Linux Agent, then open a PTY."}</pre>
          <form className="terminal-input" onSubmit={submitTerminal}>
            <input value={terminalInput} onChange={(event) => setTerminalInput(event.target.value)} disabled={!terminalId} placeholder="Command or UTF-8 terminal input" />
            <button type="submit" disabled={!terminalId || !terminalInput}>Send</button>
            <button type="button" className="secondary" disabled={!terminalId} onClick={() => void invoke("send_terminal_input", { terminalId, data: "\u0003" })}>Ctrl+C</button>
          </form>

          <div className="files-header system-header">
            <div><p className="eyebrow">SYSTEM</p><h2>{systemInfo?.hostname ?? "System information"}</h2></div>
            <button type="button" className="secondary" disabled={status.state !== "connected"} onClick={() => void invoke("request_system_info")}>Refresh</button>
          </div>
          {systemInfo && <div className="system-grid">
            <div><span>OS</span><strong>{systemInfo.operatingSystem}</strong><small>{systemInfo.kernelVersion}</small></div>
            <div><span>CPU</span><strong>{systemInfo.cpuModel}</strong><small>{systemInfo.cpuCount} logical CPUs</small></div>
            <div><span>Memory</span><strong>{formatBytes(systemInfo.usedMemoryBytes)} / {formatBytes(systemInfo.totalMemoryBytes)}</strong><small>Uptime {Math.floor(systemInfo.uptimeSeconds / 3600)}h</small></div>
            <div><span>Storage</span><strong>{systemInfo.disks.length} mounts</strong><small>{systemInfo.disks.map((disk) => disk.mountPoint).join(", ") || "None"}</small></div>
            <div><span>Network</span><strong>{systemInfo.networkInterfaces.length} interfaces</strong><small>{systemInfo.networkInterfaces.map((network) => network.name).join(", ") || "None"}</small></div>
            <div><span>GPU</span><strong>{systemInfo.gpus.map((gpu) => gpu.name).join(", ") || "Optional / not detected"}</strong><small>NVIDIA metrics use nvidia-smi when available</small></div>
          </div>}
        </section>
        </div>
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
