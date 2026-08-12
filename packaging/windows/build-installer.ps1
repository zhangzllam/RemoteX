$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
$repository = Resolve-Path (Join-Path $scriptDirectory "..\..")
$desktopDirectory = Join-Path $repository "apps\desktop"
$uiDirectory = Join-Path $desktopDirectory "ui"
$binaryDirectory = Join-Path $desktopDirectory "binaries"
$releaseConfig = Join-Path $scriptDirectory "tauri.release.conf.json"

Push-Location $repository
try {
    cargo build --locked --release -p remotex-agent
    if ($LASTEXITCODE -ne 0) { throw "Windows Agent build failed" }

    $targetTriple = (rustc --print host-tuple).Trim()
    if (-not $targetTriple.EndsWith("pc-windows-msvc")) {
        throw "Build the Windows installer with an MSVC Windows Rust toolchain"
    }
    New-Item -ItemType Directory -Force -Path $binaryDirectory | Out-Null
    $agentSource = Join-Path $repository "target\release\remotex-agent.exe"
    $agentSidecar = Join-Path $binaryDirectory "remotex-agent-$targetTriple.exe"
    Copy-Item -LiteralPath $agentSource -Destination $agentSidecar -Force

    pnpm --dir $uiDirectory install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { throw "Desktop dependency installation failed" }

    Push-Location $desktopDirectory
    try {
        & (Join-Path $uiDirectory "node_modules\.bin\tauri.cmd") build --config $releaseConfig
        if ($LASTEXITCODE -ne 0) { throw "Tauri installer build failed" }
    }
    finally {
        Pop-Location
    }
}
finally {
    Pop-Location
}

Write-Host "RemoteX NSIS installer created under target\release\bundle\nsis"
