import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Icon } from "./ui";
import { friendlyError } from "./friendly-errors";
import { remoteXApi } from "./remote-api";
import type { ServerConfig, ServerConfigState } from "./types";

type Step = "migration" | "server";

export function SetupWizard({
  state,
  onComplete,
  onLater,
}: {
  state: ServerConfigState;
  onComplete: (config: ServerConfig) => void;
  onLater: () => void;
}) {
  const [step, setStep] = useState<Step>(state.migrationConflict ? "migration" : "server");
  const [config, setConfig] = useState<ServerConfig>(state.config);
  const [advanced, setAdvanced] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");

  useEffect(() => {
    setConfig(state.config);
    setStep(state.migrationConflict ? "migration" : "server");
  }, [state]);

  function update<K extends keyof ServerConfig>(key: K, value: ServerConfig[K]) {
    setConfig((current) => ({ ...current, [key]: value }));
    setMessage("");
  }

  async function chooseCertificate() {
    const selected = await open({ multiple: false, directory: false, filters: [{ name: "Certificates", extensions: ["pem", "crt", "cer"] }] });
    if (typeof selected === "string") update("caCertificatePath", selected);
  }

  async function verifyServer() {
    setBusy(true); setMessage("Checking the secure server…");
    try {
      const result = await remoteXApi.checkServerConnection(config);
      if (!result.ok) { setMessage(result.message); return; }
      setMessage("Saving the server configuration…");
      const savedConfig = await remoteXApi.saveServerConfig(config);
      onComplete(savedConfig);
    } catch (error) { setMessage(friendlyError(error)); }
    finally { setBusy(false); }
  }

  return <div className="setup-shell">
    <div className="setup-window">
      <header className="setup-header"><span className="brand-mark"><img src="/app-icon.png" alt="" /></span><div><strong>Set up RemoteX</strong><small>Private remote access for your computers</small></div></header>

      {step === "migration" && <section className="setup-content">
        <span className="setup-symbol"><Icon name="warning" size={24} /></span><h1>Choose the server to keep</h1><p>RemoteX found different custom server settings from v1.1. Select one explicitly; neither value has been overwritten.</p>
        <div className="migration-choices">{state.candidates.map((candidate) => <button type="button" key={`${candidate.source}-${candidate.config.controlServerUrl}`} onClick={() => { setConfig(candidate.config); setStep("server"); }}><span><strong>{candidate.source}</strong><small>{candidate.config.controlServerUrl}</small></span><Icon name="chevron" /></button>)}</div>
      </section>}

      {step === "server" && <section className="setup-content">
        <span className="setup-symbol"><Icon name="server" size={24} /></span><h1>Connect to your server</h1><p>Use the HTTPS address and Relay CA certificate from your RemoteX deployment.</p>
        <label className="field-label">Server address<input autoFocus value={config.controlServerUrl} onChange={(event) => update("controlServerUrl", event.target.value)} placeholder="https://remote.example.com" autoComplete="url" /></label>
        <label className="field-label">Relay CA certificate<div className="path-picker"><input value={config.caCertificatePath} onChange={(event) => update("caCertificatePath", event.target.value)} placeholder="Select a PEM, CRT, or CER file" /><button type="button" className="secondary-button" onClick={() => void chooseCertificate()}>Choose…</button></div></label>
        <button type="button" className="disclosure-button" aria-expanded={advanced} onClick={() => setAdvanced((value) => !value)}><Icon name="chevron" size={13} />Advanced Relay fallback</button>
        {advanced && <div className="setup-advanced"><label>Relay address<input value={config.relayAddress} onChange={(event) => update("relayAddress", event.target.value)} placeholder="relay.example.com:7443" /></label><label>TLS server name<input value={config.relayServerName} onChange={(event) => update("relayServerName", event.target.value)} placeholder="relay.example.com" /></label></div>}
        {message && <p className="setup-message" role="status">{busy && <span className="spinner" />}{message}</p>}
        <div className="setup-actions"><button type="button" className="text-button" disabled={busy} onClick={onLater}>Set up later</button><button type="button" disabled={busy || !config.controlServerUrl || !config.caCertificatePath} onClick={() => void verifyServer()}>{busy ? "Working…" : "Save server"}</button></div>
      </section>}
    </div>
  </div>;
}
