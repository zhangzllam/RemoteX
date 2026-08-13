import type { ReactNode } from "react";

export type IconName =
  | "home" | "devices" | "files" | "settings" | "about" | "monitor"
  | "copy" | "shield" | "terminal" | "server" | "wifi" | "power"
  | "plus" | "trash" | "folder" | "upload" | "download" | "refresh"
  | "disconnect" | "chevron" | "check" | "warning" | "sun" | "moon" | "transfer";

const paths: Record<IconName, ReactNode> = {
  home: <><path d="m3 10 9-7 9 7"/><path d="M5 9v11h14V9"/><path d="M9 20v-6h6v6"/></>,
  devices: <><rect x="3" y="4" width="18" height="12" rx="2"/><path d="M8 20h8M12 16v4"/></>,
  files: <><path d="M3 7h7l2 2h9v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/><path d="M3 7V5a2 2 0 0 1 2-2h5l2 2h4"/></>,
  settings: <><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2.8 2.8-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.2h-4V21a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L4.2 17l.1-.1a1.7 1.7 0 0 0 .3-1.9A1.7 1.7 0 0 0 3 14H2.8v-4H3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L4.2 7 7 4.2l.1.1A1.7 1.7 0 0 0 9 4.6 1.7 1.7 0 0 0 10 3V2.8h4V3a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 7l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v4H21a1.7 1.7 0 0 0-1.6 1Z"/></>,
  about: <><circle cx="12" cy="12" r="9"/><path d="M12 11v6M12 7h.01"/></>,
  monitor: <><rect x="3" y="4" width="18" height="13" rx="2"/><path d="M8 21h8M12 17v4"/></>,
  copy: <><rect x="8" y="8" width="11" height="11" rx="2"/><path d="M16 8V5a2 2 0 0 0-2-2H5a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2h3"/></>,
  shield: <path d="M12 3 4.5 6v5.5c0 4.8 3.1 8 7.5 9.5 4.4-1.5 7.5-4.7 7.5-9.5V6Z"/>,
  terminal: <><path d="m4 7 5 5-5 5M11 17h9"/><rect x="2" y="3" width="20" height="18" rx="2"/></>,
  server: <><rect x="3" y="4" width="18" height="6" rx="2"/><rect x="3" y="14" width="18" height="6" rx="2"/><path d="M7 7h.01M7 17h.01M11 7h7M11 17h7"/></>,
  wifi: <><path d="M4 9a12 12 0 0 1 16 0M7 12.5a7.5 7.5 0 0 1 10 0M10 16a3 3 0 0 1 4 0"/><circle cx="12" cy="19" r="1" fill="currentColor" stroke="none"/></>,
  power: <><path d="M12 2v10M6.3 5.7a8 8 0 1 0 11.4 0"/></>,
  plus: <path d="M12 5v14M5 12h14"/>,
  trash: <><path d="M4 7h16M9 7V4h6v3M6 7l1 14h10l1-14M10 11v6M14 11v6"/></>,
  folder: <path d="M3 7h7l2 2h9v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>,
  upload: <><path d="M12 16V4m-5 5 5-5 5 5"/><path d="M4 15v5h16v-5"/></>,
  download: <><path d="M12 4v12m-5-5 5 5 5-5"/><path d="M4 15v5h16v-5"/></>,
  refresh: <><path d="M20 7v5h-5"/><path d="M19 12a7 7 0 1 0-2 5"/></>,
  disconnect: <><path d="M9 7V4h11v16H9v-3"/><path d="M13 12H3m4-4-4 4 4 4"/></>,
  chevron: <path d="m9 6 6 6-6 6"/>,
  check: <path d="m5 12 4 4L19 6"/>,
  warning: <><path d="M12 3 2.8 20h18.4Z"/><path d="M12 9v4M12 17h.01"/></>,
  sun: <><circle cx="12" cy="12" r="4"/><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4"/></>,
  moon: <path d="M20.5 14.2A8.5 8.5 0 0 1 9.8 3.5 8.5 8.5 0 1 0 20.5 14.2Z"/>,
  transfer: <><path d="M4 8h13m-3-3 3 3-3 3"/><path d="M20 16H7m3-3-3 3 3 3"/></>,
};

/** A dependency-free, stroke-based interface icon with consistent optical weight. */
export function Icon({ name, size = 18 }: { name: IconName; size?: number }) {
  return <svg className="icon" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{paths[name]}</svg>;
}

/** Accessible switch used for immediate boolean settings. */
export function Toggle({ checked, onChange, label, disabled = false }: { checked: boolean; onChange: (checked: boolean) => void; label: string; disabled?: boolean }) {
  return <button type="button" className="toggle" role="switch" aria-checked={checked} aria-label={label} disabled={disabled} onClick={() => onChange(!checked)}><span /></button>;
}

export function StatusPill({ tone, children }: { tone: "success" | "neutral" | "warning" | "info"; children: ReactNode }) {
  return <span className={`status-pill ${tone}`}><span className="status-dot" />{children}</span>;
}
