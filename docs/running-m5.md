# Running the M5 Development Stack

M5 extends the M4 development stack with authorized keyboard input. Use the
Relay, Agent, and Controller setup from [Running M4](running-m4.md). The same
explicit Agent setting gates both mouse and keyboard execution:

```powershell
$env:REMOTEX_ALLOW_INPUT = "true"
cargo run -p remotex-agent
```

If the variable is absent or false, video remains available but neither mouse
nor keyboard events reach Windows. Keep the Agent terminal visible so the local
user can see the active Session. Tokens, keys, keystrokes, and input payloads
are never logged.

## Controller behavior

Start the Controller as documented for M4 and connect. Click inside the remote
image to focus it; a blue focus outline indicates that supported keyboard events
are sent remotely. Clicking elsewhere, switching windows, disconnecting, or
losing the Session releases all keys tracked by both the Controller and Agent.

M5 transmits physical key positions, not text. Supported keys are A–Z, 0–9,
F1–F12, Enter, Escape, Tab, Backspace, Delete, Insert, Home, End, Page Up/Down,
arrows, Space, and left/right Shift, Control, Alt, and Windows/Super.

## Manual Windows test

1. Open Notepad on the Agent and focus it remotely.
2. Type letters, digits, spaces, Enter, Tab, and Backspace.
3. Verify arrows, Home/End, Page Up/Down, Delete, and Shift selections.
4. Verify `Ctrl+A`, `Ctrl+C`, `Ctrl+V`, and `Ctrl+Shift+T` in appropriate test
   applications.
5. Try `Alt+Tab` and Windows/Super combinations. Windows or the local WebView
   may reserve some system shortcuts; M5 sends them when the focused Controller
   receives their events and does not bypass OS restrictions.
6. Hold Control or Shift, move focus out of the remote surface, and verify the
   Agent key is released.
7. Hold a key and disconnect or stop the Relay; reconnect and verify that no key
   remains stuck.
8. Restart the Agent with `REMOTEX_ALLOW_INPUT=false`; verify keyboard and mouse
   execution are denied while video continues.

Run the Agent and target application at the same integrity level. Windows may
block `SendInput` into a higher-integrity process, and RemoteX does not bypass
that security boundary.
