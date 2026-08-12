# Running the M7 Development Stack

M7 extends the M6 stack with opt-in remote file browsing, directory creation,
upload, download, cancellation, progress, and explicit resume. Use the Relay,
video, input, and clipboard setup from [Running M6](running-m6.md).

## Configure Agent file roots and permissions

File access is disabled by default. Configure one or more explicitly named roots
and enable upload and/or download independently before starting the Agent:

```powershell
$env:REMOTEX_FILE_ROOTS = "Documents=C:\Users\User\Documents;Data=D:\Data"
$env:REMOTEX_ALLOW_FILE_UPLOAD = "true"
$env:REMOTEX_ALLOW_FILE_DOWNLOAD = "true"
cargo run -p remotex-agent
```

The Controller sees these as `/Documents` and `/Data`; it never receives the
Agent's physical root paths. Root paths must be absolute and already exist as
directories when the Agent starts. A permission set to false is enforced by the Agent
even if the Controller UI checkbox is enabled. Keep the Agent terminal visible
while the Session is active.

## Use the Files view

Before connecting, enable **Allow file upload** and/or **Allow file download**
in the Controller. After connection:

1. Select **Refresh** to list `/` and open a named root.
2. Create a folder with a single folder name.
3. For upload, enter the local source file and a full virtual destination such
   as `/Documents/inbox/archive.zip`.
4. For download, select a remote file, enter the full local destination file,
   and select **Download**.
5. Use **Cancel** on an active progress row to stop a transfer.

The MVP uses explicit path fields instead of native file pickers. Existing final
destinations are not overwritten. Uploads and downloads use a 4 MiB maximum
chunk, one chunk in flight, per-chunk SHA-256, and whole-file SHA-256.

## Resume an interrupted transfer

Copy the transfer ID shown in the progress row. Reconnect, enter the same
local/remote paths and the ID under **Interrupted transfer ID**, then select
**Resume upload** or **Resume download**. The receiving peer opens the hidden
partial for that ID and replies with its persisted offset. RemoteX seeks to that
offset instead of restarting at zero.

Explicit cancellation deletes the receiving partial. A network disconnect
closes file handles but keeps the partial so it can be resumed. If the source
changed, final SHA-256 verification fails and no completed destination is
created; cancel the old transfer before starting a new ID.

## Manual Windows validation

1. Browse each configured root and verify name, size, type, and modified time.
2. Attempt `/Documents/../../../Windows` and verify it is rejected.
3. Create a new folder and verify it appears only under the selected root.
4. Upload and download a small multilingual text file and compare SHA-256 with
   `Get-FileHash -Algorithm SHA256` on both machines.
5. Transfer a file larger than 8 MiB and verify progress advances by chunks while
   video, mouse, and keyboard remain responsive.
6. Interrupt a larger transfer, reconnect, reuse its transfer ID, and verify the
   displayed offset resumes above zero.
7. Cancel a transfer and verify no final destination is visible.
8. Disable upload or download independently on the Agent and verify the denied
   direction fails without affecting other capabilities.

M7 does not implement remote delete, rename, move, recursive operations, file
clipboard formats, or access outside configured roots.
