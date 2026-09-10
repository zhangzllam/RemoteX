import { createContext, type ReactNode, useContext, useEffect, useMemo, useState } from "react";

export type AppLanguage = "en" | "zh-CN";

const LANGUAGE_STORAGE_KEY = "remotex-language";

const ZH_CN: Record<string, string> = {
  "Secure remote access": "安全远程访问", "Main navigation": "主导航", "Home": "主页", "Devices": "设备", "Files": "文件", "Settings": "设置", "About": "关于",
  "Manage this PC and recent connections.": "管理此电脑和最近连接的设备。", "Browse transfers and remote tools.": "浏览传输和远程工具。", "Manage RemoteX preferences and behavior.": "管理 RemoteX 的偏好设置和应用行为。", "RemoteX product and update information.": "RemoteX 产品与更新信息。",
  "General": "通用", "Appearance": "个性化", "Remote Access": "远程访问", "Permissions": "权限", "Video": "视频", "Network": "网络", "Security": "安全",
  "This PC is ready": "此电脑已就绪", "Remote access starting": "正在启动远程访问", "Remote access off": "远程访问已关闭", "Remote access on": "远程访问已开启", "Remote access is off": "远程访问已关闭", "Local access off": "本机访问已关闭", "Starting…": "正在启动…", "Stopping…": "正在停止…",
  "This Device": "本机", "Share your ID only with people you trust.": "仅与您信任的人分享您的 ID。", "Your Device ID": "您的设备 ID", "Device ID": "设备 ID", "Copy": "复制", "Copied": "已复制",
  "Ready for remote connections": "可接受远程连接", "Preparing…": "正在准备…", "Preparing device ID": "正在准备设备 ID", "Remote access is starting. You can keep using RemoteX.": "正在启动远程访问。您可以继续使用 RemoteX。", "Allow connections to this computer.": "允许连接到此电脑。",
  "Connect to a Device": "连接设备", "Enter the ID shown on the other device.": "输入另一台设备上显示的 ID。", "Remote Device ID": "远程设备 ID", "Connection Options": "连接选项", "Access password": "访问密码", "Remote device confirmation is required before connecting.": "连接前需要远程设备确认。", "Recent Devices": "最近设备", "View all": "查看全部", "Devices you connect to appear here.": "连接过的设备会显示在这里。", "Use ID": "使用 ID", "Permissions are confirmed on the remote device.": "权限由远程设备确认。",
  "This Windows PC": "此 Windows 电脑", "Share this ID only with people you trust.": "仅与您信任的人分享此 ID。", "Ready": "已就绪", "Starting": "正在启动", "Access off": "访问已关闭", "Available when Remote Access is on": "开启远程访问后可用",
  "Security settings": "安全设置", "Device settings": "设备设置", "Available after Remote Access starts": "远程访问启动后可用",
  "Your device ID": "您的设备 ID", "Use this ID to connect to this PC.": "使用此 ID 连接到这台电脑。",
  "Allow remote access": "允许远程访问", "Makes this computer available through your configured server.": "允许通过已配置的服务器访问此电脑。", "Turn on remote access": "开启远程访问", "Review settings": "查看设置",
  "End-to-end encrypted": "端到端加密", "Session keys remain between your devices.": "会话密钥仅保留在您的设备之间。", "Connections require confirmation and are end-to-end encrypted.": "连接需要对方确认，并采用端到端加密。",
  "Connect to a device": "连接设备", "Enter the ID shown in RemoteX on the other device.": "输入另一台设备上 RemoteX 显示的 ID。", "Remote device ID": "远程设备 ID", "Connect": "连接", "A 9-digit RemoteX device ID.": "9 位 RemoteX 设备 ID。", "Connection options": "连接选项", "Controller name": "控制端名称", "Unattended secret": "无人值守密码", "Optional": "可选", "Connecting securely…": "正在安全连接…", "Connection failed": "连接失败", "Recent": "最近使用", "Permission-first sessions": "权限优先的会话", "The remote device controls input, clipboard, and files.": "远程设备决定是否允许输入、剪贴板和文件访问。",
  "Your local device and connections stored only on this PC.": "本机设备和连接记录仅保存在此电脑上。", "Online and ready for secure access": "在线并可安全访问", "Remote access is starting": "正在启动远程访问", "Remote access is disabled": "远程访问已禁用", "Offline": "离线", "Active session": "活动会话", "Disconnect": "断开连接", "Recent devices": "最近设备", "Created from successful connections on this PC.": "根据此电脑上的成功连接生成。", "No recent devices": "没有最近设备", "Devices appear here after your first successful connection.": "首次成功连接后，设备将显示在这里。",
  "Secure session": "安全会话", "Files & tools": "文件与工具", "Browse transfers and administer the active remote device.": "浏览传输并管理当前远程设备。", "Session active": "会话进行中", "No active session": "无活动会话", "Connect to use remote tools": "连接后使用远程工具", "File browsing, terminal access, and system details are available during an authorized session.": "授权会话期间可以浏览文件、使用终端并查看系统信息。", "Connect a device": "连接设备",
  "No device connected": "未连接设备", "Connect a device to unlock file tools": "连接设备后可使用文件工具", "Quick tools": "快捷工具", "Available during an authorized session.": "在授权会话期间可用。", "File browser": "文件浏览", "Browse remote files": "浏览远程文件", "Send files securely": "安全发送文件", "Save files locally": "保存文件到本机", "Terminal": "远程终端", "Open a remote PTY": "打开远程终端",
  "Remote files": "远程文件", "Up": "上一级", "Name": "名称", "Size": "大小", "Modified": "修改时间", "Refresh to load this directory.": "刷新以加载此目录。", "New folder": "新建文件夹", "Folder name": "文件夹名称", "Create folder": "创建文件夹", "Upload": "上传", "Local source path": "本地源路径", "Remote destination path": "远程目标路径", "Download": "下载", "Remote source path": "远程源路径", "Local destination path": "本地目标路径", "Transfers": "传输", "Cancel": "取消", "Interrupted transfer ID": "中断的传输 ID", "Resume upload": "继续上传", "Resume download": "继续下载",
  "Linux terminal": "Linux 终端", "Open an authorized remote PTY.": "打开经授权的远程 PTY。", "Open terminal": "打开终端", "Close": "关闭", "Terminal output will appear here.": "终端输出将显示在这里。", "Command or terminal input": "命令或终端输入", "Send": "发送", "System information": "系统信息", "OS, CPU, memory, storage, network, and GPU summary.": "操作系统、CPU、内存、存储、网络和 GPU 摘要。", "Refresh": "刷新", "Memory": "内存", "Not detected": "未检测到",
  "Settings marked with a switch are saved immediately.": "带开关的设置会立即保存。", "Saving…": "正在保存…", "Device name": "设备名称", "Shown to people connecting to this PC.": "向连接此电脑的人显示。", "Connection name": "连接名称", "Shown on remote devices when you connect.": "连接时显示在远程设备上。", "Start with Windows": "随 Windows 启动", "Launch RemoteX after you sign in.": "登录 Windows 后启动 RemoteX。",
  "Interface language": "界面语言", "Choose the language used throughout RemoteX.": "选择 RemoteX 全部界面使用的语言。", "English": "English", "Simplified Chinese": "简体中文", "Check now": "立即检查", "Checking…": "正在检查…",
  "Color mode": "颜色模式", "Choose how RemoteX and its Windows title bar appear.": "选择 RemoteX 界面及 Windows 标题栏的显示方式。", "System": "跟随系统", "Match Windows": "匹配 Windows 设置", "Light": "浅色", "Bright surfaces": "明亮界面", "Dark": "深色", "Dim surfaces": "深色界面", "Changes apply immediately and are saved on this PC.": "更改会立即生效并保存在此电脑上。",
  "Keyboard and mouse": "键盘和鼠标", "Allow remote input": "允许远程输入", "Plain-text clipboard": "纯文本剪贴板", "Allow clipboard synchronization": "允许同步剪贴板", "File upload": "文件上传", "Allow files to be sent to this PC": "允许向此电脑发送文件", "File download": "文件下载", "Allow files to be downloaded from this PC": "允许从此电脑下载文件", "Allowed folders": "允许的文件夹", "Remote file access stays inside these locations.": "远程文件访问仅限这些位置。", "Add folder": "添加文件夹", "No folders allowed": "未允许任何文件夹", "File permissions remain unavailable until you add one.": "添加文件夹前，文件权限不可用。",
  "Video quality": "视频质量", "Low · up to 10 FPS": "低 · 最高 10 FPS", "Balanced · up to 20 FPS": "均衡 · 最高 20 FPS", "High · up to 30 FPS": "高 · 最高 30 FPS", "Balanced is recommended for most networks.": "大多数网络建议使用均衡模式。",
  "RemoteX server": "RemoteX 服务器", "Used by this computer and every outgoing connection.": "供此电脑及所有主动连接使用。", "Relay CA certificate": "中继 CA 证书", "Select a PEM, CRT, or CER file": "选择 PEM、CRT 或 CER 文件", "Choose…": "选择…", "Advanced endpoints": "高级端点", "Relay address": "中继地址", "TLS server name": "TLS 服务器名称", "Optional managed-session fallback": "可选的托管会话回退地址", "Save and check": "保存并检查", "Run setup again": "重新运行设置向导", "Clipboard for outgoing sessions": "主动连接的剪贴板", "Request clipboard permission when connecting.": "连接时请求剪贴板权限。", "File upload for outgoing sessions": "主动连接的文件上传", "File download for outgoing sessions": "主动连接的文件下载",
  "Set up RemoteX": "设置 RemoteX", "Private remote access for your computers": "为您的电脑提供私有远程访问", "Choose the server to keep": "选择要保留的服务器", "RemoteX found different custom server settings from v1.1. Select one explicitly; neither value has been overwritten.": "RemoteX 发现了来自 v1.1 的不同自定义服务器设置。请明确选择一项；两个配置均未被覆盖。", "This device": "此设备", "Outgoing connections": "主动连接", "Connect to your server": "连接到您的服务器", "Use the HTTPS address and Relay CA certificate from your RemoteX deployment.": "使用 RemoteX 部署中的 HTTPS 地址和中继 CA 证书。", "Server address": "服务器地址", "Advanced Relay fallback": "高级中继回退设置", "Set up later": "稍后设置", "Save server": "保存服务器", "Working…": "正在处理…", "Checking the secure server…": "正在检查安全服务器…", "Saving the server configuration…": "正在保存服务器配置…", "Server setup is complete.": "服务器设置已完成。",
  "Unattended access": "无人值守访问", "Requires an explicit secret of at least 12 characters.": "需要明确设置至少 12 个字符的密码。", "Leave blank to keep the protected secret": "留空以保留受保护的密码", "At least 12 characters": "至少 12 个字符", "Apply": "应用",
  "Version": "版本", "A private remote desktop for your own Windows PCs and Linux servers, designed around explicit permissions and end-to-end encryption.": "面向个人 Windows 电脑和 Linux 服务器的私有远程桌面，采用明确授权和端到端加密。", "Private": "私有", "Self-hosted coordination": "自托管协调服务", "Responsive": "响应迅速", "Adaptive remote video": "自适应远程视频", "Capable": "功能完善", "Desktop and server tools": "桌面与服务器工具",
  "Self-hosted coordination keeps session data under your control.": "自托管协调服务让会话数据始终由您掌控。", "Adaptive remote video balances quality and latency.": "自适应远程视频兼顾画质与延迟。", "Desktop, file, clipboard, and server tools in one app.": "集成桌面、文件、剪贴板与服务器工具。", "Reliable": "安全可靠", "Signed updates and explicit authorization protect every connection.": "签名更新和明确授权保护每一次连接。",
  "Copy device ID": "复制设备 ID", "Refresh files": "刷新文件", "Settings categories": "设置分类", "Remote desktop": "远程桌面", "Remote desktop video": "远程桌面视频", "Software update available": "有可用的软件更新",
  "Waiting for the first frame": "正在等待首帧画面", "Session ended": "会话已结束", "Return Home to connect again.": "返回主页以重新连接。",
  "Diagnostics": "诊断", "Performance diagnostics": "性能诊断", "Connection": "连接", "Transport": "传输协议", "P2P Direct": "P2P 直连", "Relay": "服务器中继", "Negotiating": "正在协商", "Rendered": "渲染", "Frames": "帧", "Decode": "解码", "End to end": "端到端", "Received / rendered / dropped. No session content is logged.": "已接收 / 已渲染 / 已丢弃。不会记录会话内容。",
  "Connection quality": "连接质量", "Excellent": "优秀", "Good": "良好", "Fair": "一般", "Poor": "较差", "Measuring": "测量中",
  "Video profile": "视频模式", "Auto · adaptive": "自动 · 自适应", "Quality · up to 30 FPS": "画质 · 最高 30 FPS", "Low bandwidth · up to 10 FPS": "低带宽 · 最高 10 FPS", "Auto adapts quality to current network conditions.": "自动模式会根据当前网络状况调整画质。",
  "Bitrate": "码率", "RTT": "往返延迟", "Packet loss": "丢包率", "Send queue": "发送队列", "Capture": "采集", "Encode": "编码", "Render": "渲染", "Frame age": "画面帧龄",
  "Public endpoint discovery": "公网端点发现", "STUN address": "STUN 地址", "Optional numeric IP:port": "可选的数字 IP:端口", "Advanced self-hosted UDP mapping discovery. Relay remains available if discovery fails.": "高级自托管 UDP 映射发现；发现失败时仍会使用服务器中继。",
  "Reconnecting securely…": "正在安全地重新连接…", "Connection recovered through Relay": "已通过服务器中继恢复连接", "Connected securely through your server": "已通过您的服务器安全连接", "Connected securely over a direct path": "已通过 P2P 直连安全连接", "Connected securely over the local network": "已通过本地网络安全连接",
};

function translateText(value: string): string {
  const exact = ZH_CN[value];
  if (exact) return exact;
  if (value.startsWith("Last connected ")) return `上次连接：${value.slice(15)}`;
  if (value.startsWith("Connected for ")) return `已连接 ${value.slice(14)}`;
  return value;
}

const originalText = new WeakMap<Text, string>();
const originalElementAttributes = new WeakMap<Element, Map<string, string>>();

function localizeDocument(language: AppLanguage): () => void {
  const attributes = ["aria-label", "placeholder", "title"];
  const apply = (root: Node) => {
    const elements = root instanceof Element ? [root, ...root.querySelectorAll("*")] : root instanceof Document ? [...root.querySelectorAll("*")] : [];
    for (const element of elements) {
      if (element.closest(".terminal, [data-private]")) continue;
      for (const attribute of attributes) {
        const current = element.getAttribute(attribute);
        if (current === null) continue;
        let saved = originalElementAttributes.get(element);
        if (!saved) { saved = new Map(); originalElementAttributes.set(element, saved); }
        if (!saved.has(attribute)) saved.set(attribute, current);
        let source = saved.get(attribute) ?? current;
        const translatedSource = translateText(source);
        if (current !== source && current !== translatedSource) { source = current; saved.set(attribute, current); }
        const next = language === "zh-CN" ? translateText(source) : source;
        if (current !== next) element.setAttribute(attribute, next);
      }
    }
    const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
    let node = walker.nextNode() as Text | null;
    while (node) {
      if (!node.parentElement?.closest(".terminal, [data-private], style, script") && node.data.trim()) {
        if (!originalText.has(node)) originalText.set(node, node.data);
        let source = originalText.get(node) ?? node.data;
        const sourceTrimmed = source.trim();
        const translatedSource = source.replace(sourceTrimmed, translateText(sourceTrimmed));
        if (node.data !== source && node.data !== translatedSource) { source = node.data; originalText.set(node, source); }
        const trimmed = source.trim();
        const translated = language === "zh-CN" ? translateText(trimmed) : trimmed;
        const next = source.replace(trimmed, translated);
        if (node.data !== next) node.data = next;
      }
      node = walker.nextNode() as Text | null;
    }
  };
  apply(document);
  const observer = new MutationObserver((records) => {
    observer.disconnect();
    for (const record of records) {
      if (record.type === "childList") for (const node of record.addedNodes) apply(node);
      if (record.type === "characterData") apply(record.target.parentNode ?? record.target);
    }
    observer.observe(document.body, { childList: true, subtree: true, characterData: true });
  });
  observer.observe(document.body, { childList: true, subtree: true, characterData: true });
  return () => observer.disconnect();
}

type I18nContextValue = {
  language: AppLanguage;
  setLanguage: (language: AppLanguage) => void;
  t: (english: string, simplifiedChinese: string) => string;
};

const I18nContext = createContext<I18nContextValue | null>(null);

function initialLanguage(): AppLanguage {
  const stored = window.localStorage.getItem(LANGUAGE_STORAGE_KEY);
  if (stored === "en" || stored === "zh-CN") return stored;
  return window.navigator.language.toLowerCase().startsWith("zh") ? "zh-CN" : "en";
}

/** Provides the persisted RemoteX interface language to every UI surface. */
export function I18nProvider({ children }: { children: ReactNode }) {
  const [language, setLanguage] = useState<AppLanguage>(initialLanguage);

  useEffect(() => {
    window.localStorage.setItem(LANGUAGE_STORAGE_KEY, language);
    document.documentElement.lang = language;
    document.documentElement.dir = "ltr";
    document.title = language === "zh-CN" ? "RemoteX 远程访问" : "RemoteX Remote Access";
    return localizeDocument(language);
  }, [language]);

  const value = useMemo<I18nContextValue>(() => ({
    language,
    setLanguage,
    t: (english, simplifiedChinese) => language === "zh-CN" ? simplifiedChinese : english,
  }), [language]);

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nContextValue {
  const context = useContext(I18nContext);
  if (!context) throw new Error("useI18n must be used inside I18nProvider");
  return context;
}
