# Running the M6 Development Stack

M6 extends the M5 stack with opt-in, bidirectional plain-text clipboard sync.
Use the Relay and input setup from [Running M5](running-m5.md).

## Enable clipboard permission

Clipboard authorization is independent from mouse/keyboard authorization. On
the Agent, explicitly enable it for the development Session:

```powershell
$env:REMOTEX_ALLOW_INPUT = "true"
$env:REMOTEX_ALLOW_CLIPBOARD = "true"
cargo run -p remotex-agent
```

The default is false. Clipboard text, tokens, and encrypted payloads are never
logged. Keep the Agent terminal visible while the Session is active.

Start the Controller, select **Sync plain-text clipboard** before connecting,
then connect normally. Both sides check the clipboard every 500 ms. The checkbox
and Agent permission must both be enabled. Turning on mouse/keyboard control does
not grant clipboard access.

## Manual Windows test

1. Copy `Controller → Agent 中文测试` on the Controller and paste it into
   Notepad on the Agent.
2. Copy `Agent → Controller 日本語テスト` on the Agent and paste it on the
   Controller.
3. Set an empty text value with `Set-Clipboard -Value ""` and verify the peer
   receives an empty plain-text clipboard.
4. Rapidly copy different values on both sides. Verify synchronization settles
   and does not alternate indefinitely.
5. Disable the Controller checkbox and reconnect; verify mouse/keyboard and
   video continue while clipboard is not read or written by the Controller.
6. Reconnect with the checkbox enabled but
   `REMOTEX_ALLOW_CLIPBOARD=false` on the Agent; verify the Agent rejects remote
   clipboard application while other permitted capabilities continue.

Only UTF-8 text up to 1 MiB is synchronized. Images, HTML formatting, copied
files, and larger values are ignored or rejected. Windows may briefly lock its
clipboard while another application writes; RemoteX retries the lock and polls
again without logging the content.
