import { useState, type FormEvent } from "react";
import { Icon, Toggle } from "./ui";

export type HomeRecentDevice = {
  deviceId: string;
  connectedAt: number;
};

type HomePageProps = {
  deviceId: string;
  accessPassword: string;
  recentDevices: HomeRecentDevice[];
  localDeviceName: string;
  localDeviceId: string | null;
  remoteAccessEnabled: boolean;
  localDeviceReady: boolean;
  saving: boolean;
  copied: boolean;
  agentMessage: string;
  connectionState: string;
  connectionMessage: string;
  connectionError: string;
  connecting: boolean;
  connected: boolean;
  connectionFailed: boolean;
  onDeviceIdChange: (deviceId: string) => void;
  onAccessPasswordChange: (password: string) => void;
  onConnect: (deviceId: string) => Promise<void>;
  onViewAllDevices: () => void;
  onOpenSettings: () => void;
  onToggleRemoteAccess: (enabled: boolean) => void;
  onCopyDeviceId: () => void;
};

function formatDeviceId(value: string): string {
  return value.replace(/\D/g, "").replace(/(\d{3})(?=\d)/g, "$1 ").trim();
}

/** Focused Home composition for connecting, recents, and this computer. */
export function HomePage({
  deviceId,
  accessPassword,
  recentDevices,
  localDeviceName,
  localDeviceId,
  remoteAccessEnabled,
  localDeviceReady,
  saving,
  copied,
  agentMessage,
  connectionState,
  connectionMessage,
  connectionError,
  connecting,
  connected,
  connectionFailed,
  onDeviceIdChange,
  onAccessPasswordChange,
  onConnect,
  onViewAllDevices,
  onOpenSettings,
  onToggleRemoteAccess,
  onCopyDeviceId,
}: HomePageProps) {
  const [optionsOpen, setOptionsOpen] = useState(false);
  const normalizedDeviceId = deviceId.replace(/\D/g, "");
  const canConnect = /^\d{9}$/.test(normalizedDeviceId) && !connecting && !connected;
  const visibleRecentDevices = recentDevices.slice(0, 3);

  function submit(event: FormEvent) {
    event.preventDefault();
    if (canConnect) void onConnect(normalizedDeviceId);
  }

  function connectRecent(recentDeviceId: string) {
    onDeviceIdChange(recentDeviceId);
    void onConnect(recentDeviceId);
  }

  const localStatus = saving
    ? remoteAccessEnabled ? "Starting…" : "Stopping…"
    : localDeviceReady ? "Ready for remote connections"
      : remoteAccessEnabled ? "Remote access is starting" : "Remote access is off";

  return <div className="home-workspace">
    <section className="home-section home-connect-section" aria-labelledby="connect-device-heading">
      <div className="home-connect-layout">
        <div className="home-connect-content">
          <header className="home-section-header home-primary-header">
            <h1 id="connect-device-heading">Connect to a Device</h1>
            <p>Enter the ID shown on the other device.</p>
          </header>

          <form className="connect-form" onSubmit={submit}>
            <label className="visually-hidden" htmlFor="remote-device-id">Remote Device ID</label>
            <div className={`connect-input ${connectionError && !/^\d{9}$/.test(normalizedDeviceId) ? "invalid" : ""}`}>
              <Icon name="monitor" />
              <input
                id="remote-device-id"
                value={formatDeviceId(deviceId)}
                onChange={(event) => onDeviceIdChange(event.target.value.replace(/\D/g, "").slice(0, 9))}
                inputMode="numeric"
                autoComplete="off"
                placeholder="123 456 789"
                aria-describedby={connectionError ? "device-id-help" : undefined}
              />
              <button type="submit" disabled={!canConnect}>
                {connecting ? <span className="spinner" /> : "Connect"}
                {!connecting && <Icon name="chevron" size={14} />}
              </button>
            </div>
            {connectionError && <small id="device-id-help" className="field-error" role="alert">{connectionError}</small>}

            <button
              type="button"
              className="home-options-disclosure"
              aria-expanded={optionsOpen}
              aria-controls="home-connection-options"
              onClick={() => setOptionsOpen((current) => !current)}
            >
              <Icon name="chevron" size={13} />
              Connection Options
            </button>
            {optionsOpen && <div className="home-connection-options" id="home-connection-options">
              <label>Access password
                <input
                  type="password"
                  value={accessPassword}
                  onChange={(event) => onAccessPasswordChange(event.target.value)}
                  placeholder="Optional"
                  autoComplete="current-password"
                />
              </label>
              <p className="connection-confirmation"><Icon name="check" size={14} />Remote device confirmation is required before connecting.</p>
            </div>}
          </form>

          {(connecting || connectionFailed) && <div className={`connection-feedback ${connectionState}`} role="status">
            <span className="feedback-icon">{connecting ? <span className="spinner" /> : <Icon name="warning" size={15} />}</span>
            <span><strong>{connecting ? "Connecting securely…" : "Connection failed"}</strong><small>{connectionMessage || connectionError}</small></span>
          </div>}
        </div>
        <div className="home-connect-visual" aria-hidden="true">
          <span className="hero-star star-one">✦</span><span className="hero-star star-two">✦</span><span className="hero-star star-three">✦</span>
          <span className="connection-orbit" />
          <span className="hero-monitor hero-monitor-left"><Icon name="monitor" size={42} /></span>
          <span className="hero-transfer"><Icon name="transfer" size={23} /></span>
          <span className="hero-monitor hero-monitor-right"><Icon name="monitor" size={42} /></span>
        </div>
      </div>
    </section>

    <section className="home-section home-recent-section" aria-labelledby="recent-devices-heading">
      <div className="home-subsection-heading">
        <h2 id="recent-devices-heading">Recent Devices</h2>
        {recentDevices.length > 0 && <button type="button" className="text-button" onClick={onViewAllDevices}>View all<Icon name="chevron" size={13} /></button>}
      </div>
      {visibleRecentDevices.length > 0
        ? <div className="home-recent-list">{visibleRecentDevices.map((recentDevice, index) => <div className="home-recent-row" key={recentDevice.deviceId}>
          <span className={`recent-device-icon tone-${index % 4}`}><Icon name="monitor" size={16} /></span>
          <span className="home-recent-details"><strong>{formatDeviceId(recentDevice.deviceId)}</strong><small>{`Last connected ${new Date(recentDevice.connectedAt).toLocaleString()}`}</small></span>
          <button type="button" className="recent-connect-button" disabled={connecting || connected} onClick={() => connectRecent(recentDevice.deviceId)}>Connect<Icon name="chevron" size={13} /></button>
        </div>)}</div>
        : <p className="home-empty-note">Devices you connect to appear here.</p>}
    </section>

    <section className="home-section home-local-section" aria-labelledby="this-device-heading">
      <header className="home-subsection-heading"><h2 id="this-device-heading">This Device</h2></header>
      <div className="home-local-summary">
        <span className="home-local-device-art" aria-hidden="true"><Icon name="monitor" size={34} /></span>
        <div className="home-local-identity">
          <span><strong>{localDeviceName || "This Windows PC"}</strong><small className="home-local-badge">This device</small></span>
          <small>{localStatus}</small>
        </div>
        <div className="home-local-row home-device-id-row">
          <span><strong>Device ID</strong><small>{localDeviceId ? formatDeviceId(localDeviceId) : remoteAccessEnabled ? "Preparing…" : "Available when Remote Access is on"}</small></span>
          {localDeviceId && <button type="button" className="copy-button" aria-label="Copy device ID" onClick={onCopyDeviceId}><Icon name={copied ? "check" : "copy"} size={14} />{copied ? "Copied" : "Copy"}</button>}
        </div>
        <div className="home-local-row">
          <span><strong>Remote Access</strong><small>Allow connections to this computer.</small></span>
          <Toggle label="Allow remote access" checked={remoteAccessEnabled} disabled={saving} onChange={onToggleRemoteAccess} />
        </div>
        <button type="button" className="home-security-row" onClick={onOpenSettings}><Icon name="shield" size={16} /><span>Security settings</span><Icon name="chevron" size={14} /></button>
      </div>
      {agentMessage && <p className="inline-message success" role="status"><Icon name="check" size={14} />{agentMessage}</p>}
    </section>
    <p className="home-trust-note"><Icon name="shield" size={14} />Connections require confirmation and are end-to-end encrypted.</p>
  </div>;
}
