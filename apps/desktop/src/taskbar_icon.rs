//! Explicit taskbar group identity and artwork, separate from the tray icon.

#[cfg(windows)]
#[allow(unsafe_code)]
pub fn configure(app: &tauri::AppHandle) -> anyhow::Result<()> {
    use anyhow::Context;
    use tauri::Manager;
    use windows::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        UI::WindowsAndMessaging::{ICON_BIG, ICON_SMALL, SendMessageW, WM_GETICON, WM_SETICON},
    };

    let window = app
        .get_webview_window("main")
        .context("main window missing")?;
    let hwnd = HWND(window.hwnd()?.0);
    // Tauri/tao owns this HICON for the lifetime of the window. Borrow it for
    // ICON_BIG; do not destroy it or create a second unmanaged icon. Without
    // ICON_BIG, Explorer can fall back to a stale executable icon cache.
    unsafe {
        let icon = SendMessageW(hwnd, WM_GETICON, Some(WPARAM(ICON_SMALL as usize)), None);
        anyhow::ensure!(icon.0 != 0, "main window icon missing");
        SendMessageW(
            hwnd,
            WM_SETICON,
            Some(WPARAM(ICON_BIG as usize)),
            Some(LPARAM(icon.0)),
        );
    }
    configure_shell_group(app, hwnd)?;
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn configure_shell_group(
    app: &tauri::AppHandle,
    hwnd: windows::Win32::Foundation::HWND,
) -> anyhow::Result<()> {
    use std::hash::{Hash, Hasher};
    use tauri::Manager;
    use windows::{
        Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow},
        core::w,
    };

    const ARTWORK: &[u8] = include_bytes!("../icons/icon.ico");
    let mut fingerprint = std::collections::hash_map::DefaultHasher::new();
    ARTWORK.hash(&mut fingerprint);
    let directory = app.path().app_local_data_dir()?.join("icons");
    std::fs::create_dir_all(&directory)?;
    let icon_path = directory.join(format!("taskbar-{:016x}.ico", fingerprint.finish()));
    if std::fs::read(&icon_path).ok().as_deref() != Some(ARTWORK) {
        std::fs::write(&icon_path, ARTWORK)?;
    }
    let executable = std::env::current_exe()?;
    // A distinct, stable window ID stops Explorer reusing the old inferred
    // process group's icon. It does not change Tauri's app/config identity.
    // Content-addressed artwork also invalidates the shell cache on future edits.
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd)?;
        set_shell_string(
            &store,
            w!("System.AppUserModel.RelaunchCommand"),
            &format!("\"{}\"", executable.display()),
        )?;
        set_shell_string(
            &store,
            w!("System.AppUserModel.RelaunchDisplayNameResource"),
            "RemoteX",
        )?;
        set_shell_string(
            &store,
            w!("System.AppUserModel.RelaunchIconResource"),
            &format!("{},0", icon_path.display()),
        )?;
        set_shell_string(
            &store,
            w!("System.AppUserModel.ID"),
            "com.remotex.desktop.MainWindow",
        )?;
        store.Commit()?;
    }
    Ok(())
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn set_shell_string(
    store: &windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore,
    name: windows::core::PCWSTR,
    text: &str,
) -> windows::core::Result<()> {
    use std::mem::ManuallyDrop;
    use windows::{
        Win32::{
            Foundation::PROPERTYKEY,
            System::{
                Com::StructuredStorage::{
                    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
                },
                Variant::VT_LPWSTR,
            },
            UI::Shell::PropertiesSystem::PSGetPropertyKeyFromName,
        },
        core::PWSTR,
    };
    let mut wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    let value = PROPVARIANT {
        Anonymous: PROPVARIANT_0 {
            Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                vt: VT_LPWSTR,
                Anonymous: PROPVARIANT_0_0_0 {
                    pwszVal: PWSTR(wide.as_mut_ptr()),
                },
                ..Default::default()
            }),
        },
    };
    // SetValue copies the string synchronously. This variant only borrows the
    // Vec's allocation, so it must not be passed to PropVariantClear.
    unsafe {
        let mut key = PROPERTYKEY::default();
        PSGetPropertyKeyFromName(name, &raw mut key)?;
        store.SetValue(&raw const key, &raw const value)
    }
}

#[cfg(not(windows))]
pub fn configure(_app: &tauri::AppHandle) -> anyhow::Result<()> {
    Ok(())
}
