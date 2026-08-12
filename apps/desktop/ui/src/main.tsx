import {
  FormEvent,
  KeyboardEvent as ReactKeyboardEvent,
  PointerEvent as ReactPointerEvent,
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
  width: number;
  height: number;
  sourceTimestampMs: number;
  mimeType: string;
  data: string;
};

type ConnectionStatus = {
  state: string;
  message: string;
};

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
  const screenRef = useRef<HTMLDivElement>(null);
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
    return () => {
      void unlistenFrame.then((unlisten) => unlisten());
      void unlistenStatus.then((unlisten) => unlisten());
      void unlistenFiles.then((unlisten) => unlisten());
      if (moveAnimationFrame.current !== null) {
        cancelAnimationFrame(moveAnimationFrame.current);
      }
    };
  }, []);

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
  const imageUrl = frame ? `data:${frame.mimeType};base64,${frame.data}` : undefined;

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
          {imageUrl ? (
            <img src={imageUrl} alt="Remote desktop" draggable={false} />
          ) : (
            <div className="empty">
              <span>Remote display</span>
              <small>720p JPEG · 10–15 FPS target</small>
            </div>
          )}
          {frame && <div className="telemetry">{frame.width}×{frame.height} · frame {frame.sequence}</div>}
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
        </div>
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
