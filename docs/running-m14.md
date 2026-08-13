# Running M14: Windows installer and usability

M14 packages the Tauri Controller and Windows Agent together in a current-user
NSIS installer. A normal user does not need Cargo, Node.js, or administrator
rights after installation. WebView2 uses Tauri's supported bootstrapper mode.

## Build the installer

On Windows with stable Rust, the MSVC build tools, pnpm, and NSIS available:

```powershell
.\packaging\windows\build-installer.ps1
```

The script builds `remotex-agent.exe`, gives the sidecar the target-triple name
required by Tauri, builds the React UI, and runs the official Tauri bundler. The
result is under `target\release\bundle\nsis`. Generated sidecar binaries and
installer outputs are ignored by Git.

The installer is per-user by default. Production publishers should Authenticode
sign the executable and installer using the official Tauri Windows-signing
configuration and a protected organization certificate. Never put signing keys
in the repository.

## First use and visible operation

Open **Agent Settings** and review every field. Remote access, all optional
permissions, unattended access, and Start with Windows default to off.

1. Enter the HTTPS Control Server and Relay CA certificate path.
2. Choose a Device Name and video quality.
3. Enable only the keyboard/mouse, clipboard, and rooted file permissions needed.
4. Optionally enable unattended access and enter a 12–128 byte secret. Windows
   DPAPI protects it for the current user; the UI never reads it back.
5. Explicitly select **Enable Remote Access on this PC**, save, and start Agent.
6. Select **Start RemoteX with Windows** only if desired. This uses Tauri's
   supported autostart plugin and starts the visible tray app with `--background`.

The tray shows Agent state, Device ID, and active Session state. It provides Open,
Settings, Disconnect, Disable Remote Access, and Quit. Closing the main window
hides it to the tray; Quit explicitly stops the packaged Agent. Foreground local
authorization remains the default. During an Agent Session the tray says
**Remote session active**, and Disconnect stops the Agent immediately.

Settings are stored under the Tauri per-user application-data directory. Manual
Controller tokens, Session keys, and unattended secrets are never written to
browser local storage. The Agent identity stays per-user and its private key is
not sent to the Control Server.

## Signed application updates

RemoteX 1.2 and newer use Tauri's signed updater. The release workflow creates
the signed NSIS artifact and a static `latest.json` manifest in each GitHub
Release. Update checks and downloads can happen in the background; installation
is always explicit and is blocked while an inbound or outbound remote session
is active. See the [signed update guide](signed-updates.md) for key custody,
local builds, and release verification.
