# Windows taskbar icon fix (2026-09-10)

The installed executable contained all six updated ICO sizes, and the visible
RemoteX window returned the updated 32px icon for WM_GETICON/ICON_SMALL. However,
WM_GETICON/ICON_BIG returned null. The taskbar could therefore fall back to an
older shell icon instead of the window's current artwork.

Assigning the window-owned icon to ICON_BIG alone proved insufficient: a later
screen capture still showed the old tiny taskbar group icon despite both window
icon handles returning the updated artwork. A direct live test then assigned
explicit relaunch artwork and a distinct window AppUserModelID. A taskbar capture
confirmed the RemoteX icon became comparable in size to neighboring apps.

At startup, taskbar_icon::configure now also writes the current ICO to an
app-local, content-addressed path and sets the window's RelaunchIconResource,
RelaunchCommand, RelaunchDisplayNameResource, and stable
com.remotex.desktop.MainWindow ID. Tauri's application/configuration identifier
is unchanged. The tray artwork and its sizing are unchanged. Old pinned entries
may need repinning to follow the explicit window identity.

The window icon handle stays owned by Tauri/tao; the helper neither allocates nor
destroys icon handles. Shell string variants borrow buffers only during SetValue,
which copies their contents. Cosmetic failures are logged without preventing
startup. The icon-file build-script dependencies remain in place.

Validation: live window properties were set and the actual primary taskbar was
captured at native DPI before and after. The window-icon-only attempt remained
small; the explicit group/artwork attempt displayed correctly. Targeted Clippy
with warnings denied passed. This does not alter remote-access configuration or
require clearing the user's Explorer cache.

Reference: https://learn.microsoft.com/en-us/windows/win32/properties/props-system-appusermodel-relaunchiconresource
