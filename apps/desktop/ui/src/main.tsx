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
  relayAddress: string;
  serverName: string;
  caCertificatePath: string;
  sessionId: string;
  tokenHex: string;
  endToEndKeyHex: string;
  clipboardEnabled: boolean;
};

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
  relayAddress: "127.0.0.1:7443",
  serverName: "localhost",
  caCertificatePath: "",
  sessionId: "",
  tokenHex: "",
  endToEndKeyHex: "",
  clipboardEnabled: false,
};

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
    return () => {
      void unlistenFrame.then((unlisten) => unlisten());
      void unlistenStatus.then((unlisten) => unlisten());
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
            Relay address
            <input value={request.relayAddress} onChange={(e) => update("relayAddress", e.target.value)} required />
          </label>
          <label>
            TLS server name
            <input value={request.serverName} onChange={(e) => update("serverName", e.target.value)} required />
          </label>
          <label>
            CA certificate path
            <input value={request.caCertificatePath} onChange={(e) => update("caCertificatePath", e.target.value)} required />
          </label>
          <label>
            Session ID
            <input value={request.sessionId} onChange={(e) => update("sessionId", e.target.value)} required />
          </label>
          <label>
            One-time controller token
            <input type="password" value={request.tokenHex} onChange={(e) => update("tokenHex", e.target.value)} minLength={64} maxLength={64} required />
          </label>
          <label>
            End-to-end session key
            <input type="password" value={request.endToEndKeyHex} onChange={(e) => update("endToEndKeyHex", e.target.value)} minLength={64} maxLength={64} required />
          </label>
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
          <div className="actions">
            <button type="submit" disabled={connected}>Connect</button>
            <button type="button" className="secondary" onClick={disconnect} disabled={!connected}>Disconnect</button>
          </div>
        </form>

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
      </section>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);
