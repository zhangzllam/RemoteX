import { FormEvent, useEffect, useState } from "react";
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
};

const initialRequest: ConnectRequest = {
  relayAddress: "127.0.0.1:7443",
  serverName: "localhost",
  caCertificatePath: "",
  sessionId: "",
  tokenHex: "",
  endToEndKeyHex: "",
};

function App() {
  const [request, setRequest] = useState(initialRequest);
  const [status, setStatus] = useState<ConnectionStatus>({
    state: "disconnected",
    message: "Not connected",
  });
  const [frame, setFrame] = useState<VideoFrame | null>(null);

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
    };
  }, []);

  async function connect(event: FormEvent) {
    event.preventDefault();
    setFrame(null);
    await invoke("connect_remote", { request });
  }

  async function disconnect() {
    await invoke("disconnect_remote");
  }

  function update(field: keyof ConnectRequest, value: string) {
    setRequest((current) => ({ ...current, [field]: value }));
  }

  const connected = status.state === "connected" || status.state === "connecting";
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
          <div className="actions">
            <button type="submit" disabled={connected}>Connect</button>
            <button type="button" className="secondary" onClick={disconnect} disabled={!connected}>Disconnect</button>
          </div>
        </form>

        <div className="screen" aria-live="polite">
          {imageUrl ? (
            <img src={imageUrl} alt="Remote desktop" />
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
