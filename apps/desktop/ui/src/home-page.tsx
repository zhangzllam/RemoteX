import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { Icon, StatusPill, Toggle } from "./ui";
import { useI18n } from "./i18n";

export type HomeRecentDevice = { deviceId: string; connectedAt: number };
type HomePageProps = {
  deviceId: string; accessPassword: string; accessPasswordPanel: ReactNode; passwordAccessEnabled: boolean;
  recentDevices: HomeRecentDevice[]; localDeviceName: string; localDeviceId: string | null;
  remoteAccessEnabled: boolean; localDeviceReady: boolean; saving: boolean; copied: boolean;
  agentMessage: string; connectionState: string; connectionMessage: string; connectionError: string;
  connecting: boolean; connected: boolean; connectionFailed: boolean;
  onDeviceIdChange: (value: string) => void; onAccessPasswordChange: (value: string) => void;
  onConnect: (value: string) => Promise<void>; onViewAllDevices: () => void; onOpenSettings: () => void;
  onToggleRemoteAccess: (value: boolean) => void; onCopyDeviceId: () => void;
};
function formatDeviceId(value: string) { return value.replace(/\D/g, "").replace(/(\d{3})(?=\d)/g, "$1 ").trim(); }

/** Receive and initiate connections are separate, equally discoverable tasks. */
export function HomePage(props: HomePageProps) {
  const { t, language } = useI18n();
  const [showRemotePassword, setShowRemotePassword] = useState(false);
  useEffect(() => {
    const hide = () => setShowRemotePassword(false);
    const onVisibility = () => { if (document.hidden) hide(); };
    window.addEventListener("blur", hide);
    document.addEventListener("visibilitychange", onVisibility);
    return () => { window.removeEventListener("blur", hide); document.removeEventListener("visibilitychange", onVisibility); };
  }, []);
  useEffect(() => setShowRemotePassword(false), [props.deviceId]);
  const id = props.deviceId.replace(/\D/g, "");
  const valid = /^\d{9}$/.test(id);
  const isSelf = Boolean(props.localDeviceId && id === props.localDeviceId.replace(/\D/g, ""));
  const canConnect = valid && !isSelf && !props.connecting && !props.connected;
  function submit(event: FormEvent) { event.preventDefault(); if (canConnect) void props.onConnect(id); }
  function selectRecent(value: string) {
    if (value !== id) props.onAccessPasswordChange("");
    props.onDeviceIdChange(value);
    document.getElementById("remote-access-password")?.focus();
  }
  return <div className="home-workspace refined-home">
    <header className="workspace-heading"><div><span className="eyebrow">{t("YOUR REMOTE WORKSPACE", "您的远程工作空间")}</span><h1>{t("Remote connections", "远程连接")}</h1><p>{t("Access another computer, or make this one available.", "连接另一台电脑，或允许其他设备访问本机。")}</p></div><StatusPill tone={props.localDeviceReady ? "success" : props.remoteAccessEnabled ? "warning" : "neutral"}>{props.localDeviceReady ? t("Ready to receive", "可接受连接") : props.remoteAccessEnabled ? t("Starting…", "正在启动…") : t("Remote access off", "远程访问已关闭")}</StatusPill></header>
    <div className="home-connection-grid">
      <section className="home-section receive-card" aria-labelledby="this-device-heading">
        <div className="task-card-heading"><span className="task-icon mint"><Icon name="monitor" size={23} /></span><div><span className="eyebrow">{t("THIS COMPUTER", "本机")}</span><h2 id="this-device-heading">{t("Allow a connection", "允许连接此设备")}</h2></div><Toggle label={t("Allow remote access", "允许远程访问")} checked={props.remoteAccessEnabled} disabled={props.saving} onChange={props.onToggleRemoteAccess} /></div>
        <div className="local-machine-name"><i className={props.localDeviceReady ? "status-dot success" : "status-dot"} /><strong>{props.localDeviceName || t("This Windows PC", "此 Windows 电脑")}</strong></div>
        <div className="device-credential"><div className="credential-heading"><span>{t("Your device ID", "您的设备 ID")}</span><small>{t("Share to connect", "分享以连接")}</small></div><div className="credential-value"><strong className="device-id-display">{props.localDeviceId ? formatDeviceId(props.localDeviceId) : "— — —"}</strong><button type="button" className="icon-button" disabled={!props.localDeviceId} onClick={props.onCopyDeviceId} aria-label={t("Copy device ID", "复制设备 ID")} title={t("Copy device ID", "复制设备 ID")}><Icon name={props.copied ? "check" : "copy"} size={18} /></button></div></div>
        {props.accessPasswordPanel}
        <div className="access-policy-note"><Icon name="shield" size={16} /><p>{props.passwordAccessEnabled ? t("People with your password can connect with the permissions you allow.", "持有密码的人可以按您允许的权限连接本机。") : t("First activation creates a password. Switch to approval-only in Security settings.", "首次开启将生成连接密码，也可在安全设置中改为仅手动确认。")}</p></div>
        <button type="button" className="card-footer-link" onClick={props.onOpenSettings}><Icon name="settings" size={15} />{t("Permissions & security", "权限与安全")}<Icon name="chevron" size={13} /></button>
      </section>
      <section className="home-section send-card" aria-labelledby="connect-device-heading">
        <div className="task-card-heading"><span className="task-icon blue"><Icon name="transfer" size={23} /></span><div><span className="eyebrow">{t("ANOTHER COMPUTER", "另一台电脑")}</span><h2 id="connect-device-heading">{t("Start a connection", "发起远程连接")}</h2></div></div>
        <p className="task-description">{t("Enter the details shown in RemoteX on the other computer.", "输入另一台电脑上 RemoteX 显示的连接信息。")}</p>
        <form className="remote-connect-form" onSubmit={submit}>
          <label htmlFor="remote-device-id">{t("Remote device ID", "远程设备 ID")}</label>
          <div className="remote-field"><Icon name="monitor" size={18} /><input id="remote-device-id" value={formatDeviceId(props.deviceId)} onChange={(event) => { props.onDeviceIdChange(event.target.value.replace(/\D/g, "").slice(0, 9)); props.onAccessPasswordChange(""); }} inputMode="numeric" autoComplete="off" placeholder="123 456 789" aria-invalid={isSelf} aria-describedby={isSelf ? "self-connection-error" : undefined} /></div>
          <label htmlFor="remote-access-password">{t("Connection password", "连接密码")}<small>{t("Or request approval", "或请求对方确认")}</small></label>
          <div className="remote-field"><Icon name="key" size={17} /><input id="remote-access-password" type={showRemotePassword ? "text" : "password"} value={props.accessPassword} onChange={(event) => props.onAccessPasswordChange(event.target.value)} autoComplete="off" maxLength={128} placeholder={t("Leave blank to request approval", "留空以请求对方手动确认")} /><button type="button" className="icon-button" onClick={() => setShowRemotePassword(!showRemotePassword)} aria-label={t("Show or hide remote password", "显示或隐藏远程密码")} aria-pressed={showRemotePassword}><Icon name="eye" size={17} /></button></div>
          {isSelf && <small id="self-connection-error" className="field-error" role="alert">{t("Enter another computer's ID, not this computer's ID.", "请输入另一台电脑的 ID，不能连接本机。")}</small>}
          <button className="connect-primary" type="submit" disabled={!canConnect}>{props.connecting ? <><span className="spinner" />{t("Connecting…", "正在连接…")}</> : <>{t("Connect to device", "连接设备")}<Icon name="chevron" size={16} /></>}</button>
          <p className="connect-security-note"><Icon name="shield" size={14} />{t("Encrypted transport · Your permissions, your choice", "加密传输 · 访问权限由您掌控")}</p>
        </form>
        {(props.connecting || props.connectionFailed || props.connectionError) && <div className={"connection-feedback " + props.connectionState} role={props.connectionFailed || props.connectionError ? "alert" : "status"}><span className="feedback-icon">{props.connecting ? <span className="spinner" /> : <Icon name="warning" size={16} />}</span><span>{props.connectionError || props.connectionMessage}</span></div>}
      </section>
    </div>
    {props.agentMessage && <p className="inline-message success" role="status"><Icon name="check" size={14} />{props.agentMessage}</p>}
    <section className="home-section home-recent-section" aria-labelledby="recent-devices-heading">
      <div className="home-subsection-heading"><div><h2 id="recent-devices-heading">{t("Recent connections", "最近连接")}</h2><p className="section-caption">{t("Pick up where you left off.", "快速返回您熟悉的设备。")}</p></div><button type="button" className="text-button" onClick={props.onViewAllDevices}>{t("All devices", "全部设备")}<Icon name="chevron" size={13} /></button></div>
      {props.recentDevices.length ? <div className="home-recent-list">{props.recentDevices.slice(0, 3).map((device, index) => <div className="home-recent-row" key={device.deviceId}><span className={"recent-device-icon tone-" + index % 4}><Icon name="monitor" size={21} /></span><span className="home-recent-details"><strong>{formatDeviceId(device.deviceId)}</strong><small>{t("Last connected", "上次连接")} · {new Date(device.connectedAt).toLocaleString(language)}</small></span><button type="button" className="secondary-button" disabled={props.connecting || props.connected} onClick={() => selectRecent(device.deviceId)}>{t("Select device", "选择设备")}<Icon name="chevron" size={13} /></button></div>)}</div> : <div className="recent-empty"><span className="task-icon blue"><Icon name="devices" size={24} /></span><div><strong>{t("Your next connection starts here", "从第一次连接开始")}</strong><p>{t("Successfully connected devices will appear here. No account needed.", "成功连接的设备会显示在这里，无需注册账户。")}</p></div></div>}
    </section>
  </div>;
}
