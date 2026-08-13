import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type DownloadEvent, type Update } from "@tauri-apps/plugin-updater";
import { useEffect, useRef, useState } from "react";
import { useI18n } from "./i18n";
import { Icon, Toggle } from "./ui";

const IS_TAURI = typeof (window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ === "object";
const AUTOMATIC_UPDATES_KEY = "remotex-automatic-updates";

type UpdatePhase =
  | "idle"
  | "checking"
  | "current"
  | "available"
  | "downloading"
  | "ready"
  | "installing"
  | "error";

type UpdateState = {
  phase: UpdatePhase;
  version?: string;
  notes?: string;
  date?: string;
  downloaded: number;
  total?: number;
  message?: string;
  lastChecked?: number;
};

type UseAppUpdaterOptions = {
  activeRemoteSession: boolean;
  onAgentStopped: () => void;
};

type AgentInstallStatus = {
  running: boolean;
  sessionId: string | null;
};

function loadAutomaticUpdates(): boolean {
  return window.localStorage.getItem(AUTOMATIC_UPDATES_KEY) !== "false";
}

function friendlyUpdateError(error: unknown, fallback = "The update service could not be reached."): string {
  const message = String(error).replace(/^Error:\s*/i, "").trim();
  return message || fallback;
}

function formatBytes(value: number): string {
  if (value < 1024) return `${value} B`;
  const units = ["KB", "MB", "GB"];
  let size = value;
  let unit = -1;
  do { size /= 1024; unit += 1; } while (size >= 1024 && unit < units.length - 1);
  return `${size >= 10 ? size.toFixed(0) : size.toFixed(1)} ${units[unit]}`;
}

function checkedLabel(timestamp: number | undefined, language: string, prefix: string, empty: string): string {
  if (!timestamp) return empty;
  return `${prefix}${new Intl.DateTimeFormat(language, { hour: "numeric", minute: "2-digit" }).format(timestamp)}`;
}

export function useAppUpdater({ activeRemoteSession, onAgentStopped }: UseAppUpdaterOptions) {
  const { t } = useI18n();
  const [appVersion, setAppVersion] = useState("1.2.0");
  const [automaticUpdates, setAutomaticUpdates] = useState(loadAutomaticUpdates);
  const [state, setState] = useState<UpdateState>({ phase: "idle", downloaded: 0 });
  const updateRef = useRef<Update | null>(null);
  const automaticCheckStarted = useRef(false);
  const operationRunning = useRef(false);

  useEffect(() => {
    if (!IS_TAURI) return;
    void getVersion().then(setAppVersion).catch(() => undefined);
  }, []);

  useEffect(() => {
    window.localStorage.setItem(AUTOMATIC_UPDATES_KEY, String(automaticUpdates));
  }, [automaticUpdates]);

  async function downloadUpdate(update = updateRef.current): Promise<void> {
    if (!update || operationRunning.current) return;
    operationRunning.current = true;
    let downloaded = 0;
    let total: number | undefined;
    setState((current) => ({ ...current, phase: "downloading", downloaded: 0, total: undefined, message: undefined }));
    try {
      await update.download((event: DownloadEvent) => {
        if (event.event === "Started") {
          total = event.data.contentLength;
          setState((current) => ({ ...current, phase: "downloading", downloaded: 0, total }));
        } else if (event.event === "Progress") {
          downloaded += event.data.chunkLength;
          setState((current) => ({ ...current, phase: "downloading", downloaded, total }));
        } else {
          setState((current) => ({ ...current, phase: "ready", downloaded, total }));
        }
      }, { timeout: 10 * 60 * 1_000 });
      setState((current) => ({ ...current, phase: "ready", downloaded, total }));
    } catch (error) {
      setState((current) => ({ ...current, phase: "error", message: friendlyUpdateError(error) }));
    } finally {
      operationRunning.current = false;
    }
  }

  async function checkForUpdates(manual = true): Promise<void> {
    if (!IS_TAURI) {
      if (manual) setState((current) => ({ ...current, phase: "error", message: t("Update checks are available in the installed RemoteX app.", "只有安装后的 RemoteX 应用可以检查更新。") }));
      return;
    }
    if (operationRunning.current) return;
    operationRunning.current = true;
    setState((current) => ({ ...current, phase: "checking", message: undefined }));
    try {
      const update = await check({ timeout: 30_000 });
      const now = Date.now();
      if (!update) {
        if (updateRef.current) await updateRef.current.close();
        updateRef.current = null;
        setState({ phase: "current", downloaded: 0, lastChecked: now });
        return;
      }
      if (updateRef.current) await updateRef.current.close();
      updateRef.current = update;
      setState({
        phase: "available",
        version: update.version,
        notes: update.body,
        date: update.date,
        downloaded: 0,
        lastChecked: now,
      });
      operationRunning.current = false;
      if (automaticUpdates && !manual) await downloadUpdate(update);
    } catch (error) {
      setState((current) => ({ ...current, phase: "error", message: friendlyUpdateError(error), lastChecked: Date.now() }));
    } finally {
      operationRunning.current = false;
    }
  }

  async function installUpdate(): Promise<void> {
    if (!updateRef.current || operationRunning.current) return;
    let agentStatus: AgentInstallStatus;
    try {
      agentStatus = await invoke<AgentInstallStatus>("agent_status");
    } catch (error) {
      setState((current) => ({ ...current, phase: "ready", message: `Could not confirm the remote session state: ${friendlyUpdateError(error)}` }));
      return;
    }
    if (activeRemoteSession || agentStatus.sessionId) {
      setState((current) => ({ ...current, phase: "ready", message: t("End the active remote session before installing this update.", "请先结束当前远程会话，再安装此更新。") }));
      return;
    }
    operationRunning.current = true;
    setState((current) => ({ ...current, phase: "installing", message: undefined }));
    try {
      if (agentStatus.running) {
        await invoke("stop_agent");
        onAgentStopped();
      }
      await updateRef.current.install();
      await relaunch();
    } catch (error) {
      setState((current) => ({ ...current, phase: "error", message: friendlyUpdateError(error) }));
      operationRunning.current = false;
    }
  }

  useEffect(() => {
    if (!automaticUpdates || !IS_TAURI || automaticCheckStarted.current) return;
    const timeout = window.setTimeout(() => {
      automaticCheckStarted.current = true;
      void checkForUpdates(false);
    }, 2_000);
    return () => window.clearTimeout(timeout);
  }, [automaticUpdates]);

  return {
    appVersion,
    automaticUpdates,
    setAutomaticUpdates,
    state,
    activeRemoteSession,
    checkForUpdates,
    downloadUpdate,
    installUpdate,
  };
}

export type AppUpdater = ReturnType<typeof useAppUpdater>;

export function AppUpdaterSettings({ updater }: { updater: AppUpdater }) {
  const { language, t } = useI18n();
  const busy = updater.state.phase === "checking" || updater.state.phase === "downloading" || updater.state.phase === "installing";
  return <>
    <div className="setting-row">
      <span><strong>{t("Automatic updates", "自动更新")}</strong><small>{t("Check and download signed updates in the background.", "在后台检查并下载已签名的更新。")}</small></span>
      <Toggle label={t("Automatic updates", "自动更新")} checked={updater.automaticUpdates} onChange={updater.setAutomaticUpdates} />
    </div>
    <div className="setting-row update-setting-row">
      <span><strong>{t("Software update", "软件更新")}</strong><small>{checkedLabel(updater.state.lastChecked, language, t("Last checked ", "上次检查："), t("Not checked yet", "尚未检查"))}</small></span>
      <button type="button" className="secondary-button" disabled={busy} onClick={() => void updater.checkForUpdates(true)}>
        {updater.state.phase === "checking" ? <><span className="spinner" />Checking…</> : <><Icon name="refresh" size={15} />Check now</>}
      </button>
    </div>
  </>;
}

export function AppUpdatePanel({ updater }: { updater: AppUpdater }) {
  const { t } = useI18n();
  const { state } = updater;
  const percent = state.total ? Math.min(100, Math.round((state.downloaded / state.total) * 100)) : undefined;
  const releaseNotes = state.notes?.trim();
  const installBlocked = state.phase === "ready" && updater.activeRemoteSession;
  const title = state.version ? `RemoteX ${state.version}` : t("Software Update", "软件更新");

  return <section className={`update-panel phase-${state.phase}`} aria-live="polite">
    <span className="update-panel-icon">
      {state.phase === "checking" || state.phase === "downloading" || state.phase === "installing" ? <span className="spinner" /> : <Icon name={state.phase === "current" ? "check" : state.phase === "error" ? "warning" : "download"} size={19} />}
    </span>
    <div className="update-panel-content">
      <div className="update-panel-heading">
        <span>
          <strong>{title}</strong>
          <small>
            {state.phase === "idle" && t("Signed updates are delivered securely from GitHub Releases.", "签名更新通过 GitHub Releases 安全交付。")}
            {state.phase === "checking" && t("Checking for a newer signed version…", "正在检查新的签名版本…")}
            {state.phase === "current" && t("RemoteX is up to date.", "RemoteX 已是最新版本。")}
            {state.phase === "available" && t("A signed update is ready to download.", "已有签名更新可供下载。")}
            {state.phase === "downloading" && (state.total ? `${formatBytes(state.downloaded)} of ${formatBytes(state.total)}` : `${formatBytes(state.downloaded)} downloaded`)}
            {state.phase === "ready" && (installBlocked ? t("End the active session before installing.", "请先结束当前会话再安装。") : t("Downloaded and verified. Remote access will pause briefly.", "更新已下载并验证，远程访问将短暂停止。"))}
            {state.phase === "installing" && t("Installing the update and restarting RemoteX…", "正在安装更新并重启 RemoteX…")}
            {state.phase === "error" && (state.message || t("The update could not be completed.", "无法完成更新。"))}
          </small>
        </span>
        <div className="update-actions">
          {(state.phase === "idle" || state.phase === "current" || state.phase === "error") && <button type="button" className="secondary-button" onClick={() => void updater.checkForUpdates(true)}><Icon name="refresh" size={15} />{state.phase === "error" ? t("Try again", "重试") : t("Check for updates", "检查更新")}</button>}
          {state.phase === "available" && <button type="button" onClick={() => void updater.downloadUpdate()}><Icon name="download" size={15} />{t("Download update", "下载更新")}</button>}
          {state.phase === "ready" && <button type="button" disabled={installBlocked} onClick={() => void updater.installUpdate()}><Icon name="refresh" size={15} />{t("Restart and install", "重启并安装")}</button>}
        </div>
      </div>
      {state.phase === "downloading" && <div className="update-progress"><progress aria-label="Update download progress" max={state.total || 1} value={state.total ? state.downloaded : undefined} /><span>{percent === undefined ? "Downloading…" : `${percent}%`}</span></div>}
      {(state.phase === "available" || state.phase === "ready") && releaseNotes && <details className="release-notes"><summary>What’s new</summary><p>{releaseNotes.slice(0, 2_000)}</p></details>}
      {state.phase === "ready" && state.message && <p className="update-warning"><Icon name="warning" size={14} />{state.message}</p>}
    </div>
  </section>;
}
