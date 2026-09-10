# Packaged Agent sidecar

`packaging/windows/build-installer.ps1` builds the Windows Agent and copies the
target-suffixed executable into this directory before invoking the Tauri bundler.
Generated executables are ignored and must never be committed.
