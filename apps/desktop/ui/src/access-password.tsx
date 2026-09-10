import { useEffect, useRef, useState } from "react";
import { remoteXApi } from "./remote-api";
import { useI18n } from "./i18n";
import { Icon } from "./ui";

type Props = {
  configured: boolean;
  enabled: boolean;
  busy: boolean;
  activeSession: boolean;
  onChanged: () => Promise<void>;
};

/** Passwords stay out of settings snapshots, history and localStorage. */
export function AccessPassword({ configured, enabled, busy, activeSession, onChanged }: Props) {
  const { t } = useI18n();
  const [password, setPassword] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");
  const [confirmation, setConfirmation] = useState("");
  const [pending, setPending] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");
  const revealEpoch = useRef(0);
  const disabled = pending || busy;
  const changeDisabled = disabled || activeSession;
  const byteLength = new TextEncoder().encode(draft).length;
  const valid = byteLength >= 12 && byteLength <= 128 && !/[\u0000-\u001f\u007f-\u009f]/.test(draft) && draft === confirmation;

  useEffect(() => { revealEpoch.current++; setPassword(null); setDraft(""); setConfirmation(""); setEditing(false); }, [configured, enabled]);
  useEffect(() => {
    const hide = () => { revealEpoch.current++; setPassword(null); setDraft(""); setConfirmation(""); };
    const onVisibility = () => { if (document.hidden) hide(); };
    window.addEventListener("blur", hide);
    document.addEventListener("visibilitychange", onVisibility);
    return () => { revealEpoch.current++; window.removeEventListener("blur", hide); document.removeEventListener("visibilitychange", onVisibility); };
  }, []);

  async function reveal(copy = false) {
    const epoch = ++revealEpoch.current;
    setPending(true); setError(""); setMessage("");
    try {
      const value = await remoteXApi.revealAccessPassword();
      if (epoch !== revealEpoch.current) return;
      if (!value) throw new Error("missing password");
      if (copy) {
        await navigator.clipboard.writeText(value);
        setMessage(t("Password copied. Share it only with someone you trust.", "密码已复制，请只分享给信任的人。"));
      } else setPassword(value);
    } catch { setError(t("Could not read or copy the password. Try again.", "无法读取或复制密码，请重试。")); }
    finally { setPending(false); }
  }

  async function change(value: string | null) {
    if (changeDisabled) return;
    revealEpoch.current++;
    setPending(true); setError(""); setMessage(""); setPassword(null);
    try {
      await remoteXApi.updateAccessPassword(value);
      setDraft(""); setConfirmation(""); setEditing(false);
      await onChanged();
      setMessage(t("Password updated. The previous password no longer works for new connections.", "密码已更新，旧密码无法再建立新连接。"));
    } catch { setError(t("Could not update the password. End any incoming session, then try again.", "无法更新密码，请先结束正在进行的被控会话，然后重试。")); }
    finally { setPending(false); }
  }

  return <div className="access-password" data-private>
    <div className="credential-heading"><span><Icon name="key" size={15} />{t("Connection password", "连接密码")}</span><small>{enabled ? t("Password access", "密码访问") : t("Approval required", "需手动确认")}</small></div>
    <div className="credential-value password-value">
      <input aria-label={t("Local connection password", "本机连接密码")} readOnly type={password ? "text" : "password"} value={configured ? password ?? "••••••••••••••••" : ""} placeholder={t("Generated when access starts", "开启访问后生成")} autoComplete="off" spellCheck={false} />
      <button type="button" className="icon-button" disabled={!configured || disabled} aria-label={password ? t("Hide password", "隐藏密码") : t("Show password", "显示密码")} aria-pressed={Boolean(password)} title={password ? t("Hide password", "隐藏密码") : t("Show password", "显示密码")} onClick={() => password ? setPassword(null) : void reveal()}><Icon name="eye" size={17} /></button>
      <button type="button" className="icon-button" disabled={!configured || disabled} aria-label={t("Copy password", "复制密码")} title={t("Copy password", "复制密码")} onClick={() => void reveal(true)}><Icon name="copy" size={16} /></button>
    </div>
    <div className="password-actions">
      <button type="button" className="text-button" disabled={disabled || activeSession} onClick={() => void change(null)}><Icon name="refresh" size={14} />{t("Generate another", "换一组密码")}</button>
      <button type="button" className="text-button" disabled={disabled || activeSession} aria-expanded={editing} onClick={() => { setEditing(!editing); setDraft(""); setConfirmation(""); setError(""); }}><Icon name="edit" size={14} />{t("Custom password", "自定义密码")}</button>
    </div>
    {editing && <div className="password-editor" onKeyDown={(event) => {
      if (event.key === "Escape") { event.preventDefault(); setEditing(false); setDraft(""); setConfirmation(""); }
      if (event.key === "Enter") { event.preventDefault(); if (valid && !changeDisabled) void change(draft); }
    }}>
      <label>{t("New password", "新密码")}<input autoFocus type="password" autoComplete="new-password" value={draft} maxLength={128} onChange={(event) => setDraft(event.target.value)} /></label>
      <label>{t("Confirm password", "确认密码")}<input type="password" autoComplete="new-password" value={confirmation} maxLength={128} onChange={(event) => setConfirmation(event.target.value)} aria-invalid={Boolean(confirmation && confirmation !== draft)} /></label>
      <small>{t("Use a long password, such as 12 or more letters, numbers and symbols. Saving replaces the previous password.", "建议使用至少 12 位字母、数字和符号组合；保存后旧密码失效。")}</small>
      {draft && (byteLength < 12 || byteLength > 128 || /[\u0000-\u001f\u007f-\u009f]/.test(draft)) && <small className="field-error">{t("Password is too short, too long, or contains unsupported characters.", "密码过短、过长或包含不支持的字符，请调整。")}</small>}
      {confirmation && confirmation !== draft && <small className="field-error">{t("Passwords do not match.", "两次输入的密码不一致。")}</small>}
      <div className="password-actions"><button type="button" disabled={!valid || changeDisabled} onClick={() => void change(draft)}>{t("Save password", "保存密码")}</button><button type="button" className="secondary-button" disabled={disabled} onClick={() => { setEditing(false); setDraft(""); setConfirmation(""); }}>{t("Cancel", "取消")}</button></div>
    </div>}
    {activeSession && <small>{t("End the incoming session before changing the password.", "请先结束被控会话，再修改密码。")}</small>}
    {message && <p className="password-feedback" role="status">{message}</p>}
    {error && <p className="field-error" role="alert">{error}</p>}
  </div>;
}
