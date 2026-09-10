#![allow(unsafe_code)]

use remotex_protocol::{IncomingSessionRequest, SessionId, SessionPermissions};
use windows::{
    Win32::UI::WindowsAndMessaging::{
        IDYES, MB_ICONQUESTION, MB_SETFOREGROUND, MB_YESNO, MessageBoxW,
    },
    core::PCWSTR,
};

pub fn confirm_incoming_session(
    incoming: &IncomingSessionRequest,
    permissions: SessionPermissions,
) -> bool {
    let message = format!(
        "RemoteX incoming connection\n\nController: {}\nSession: {}\n\nPermissions:\n{}\nSelect Yes to accept or No to reject.",
        incoming.controller_name,
        incoming.session_id,
        permission_summary(permissions),
    );
    let title = "RemoteX - Authorization required";
    let message = wide(&message);
    let title = wide(title);
    // SAFETY: Both strings are valid, NUL-terminated UTF-16 buffers that remain alive for the
    // duration of this synchronous call. No owner window handle is required for a foreground
    // local-consent dialog.
    (unsafe {
        MessageBoxW(
            None,
            PCWSTR(message.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_YESNO | MB_ICONQUESTION | MB_SETFOREGROUND,
        )
    }) == IDYES
}

pub fn set_active_session_title(session_id: Option<SessionId>) {
    match session_id {
        Some(session_id) => {
            println!("\n============================================================");
            println!(" RemoteX remote session active: {session_id}");
            println!(" Press Ctrl+C in this window to disconnect immediately.");
            println!("============================================================\n");
        }
        None => println!("RemoteX remote session is no longer active."),
    }
}

fn permission_summary(permissions: SessionPermissions) -> String {
    [
        ("View screen", permissions.view_desktop),
        ("Keyboard and mouse", permissions.control_input),
        ("Clipboard", permissions.clipboard),
        ("File upload", permissions.file_upload),
        ("File download", permissions.file_download),
    ]
    .into_iter()
    .map(|(name, enabled)| format!("[{}] {name}", if enabled { 'x' } else { ' ' }))
    .collect::<Vec<_>>()
    .join("\n")
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_summary_lists_independent_grants() {
        let summary = permission_summary(SessionPermissions {
            view_desktop: true,
            control_input: false,
            clipboard: true,
            file_upload: false,
            file_download: true,
            terminal: false,
            system_info: false,
        });
        assert!(summary.contains("[x] View screen"));
        assert!(summary.contains("[ ] Keyboard and mouse"));
        assert!(summary.contains("[x] Clipboard"));
    }
}
