$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
$repository = Resolve-Path (Join-Path $scriptDirectory "..\..")
$desktopDirectory = Join-Path $repository "apps\desktop"
$uiDirectory = Join-Path $desktopDirectory "ui"
$binaryDirectory = Join-Path $desktopDirectory "binaries"
$releaseConfig = Join-Path $scriptDirectory "tauri.release.conf.json"
$generatedSigningConfig = $null

function Assert-WindowsGuiSubsystem([string]$Path) {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    $peOffset = [BitConverter]::ToInt32($bytes, 0x3c)
    if ([BitConverter]::ToUInt32($bytes, $peOffset) -ne 0x00004550) {
        throw "$Path is not a Windows PE executable"
    }
    $subsystem = [BitConverter]::ToUInt16($bytes, $peOffset + 24 + 0x44)
    if ($subsystem -ne 2) {
        throw "$Path must use the Windows GUI subsystem to avoid a console window"
    }
}

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
    Assert-WindowsGuiSubsystem $agentSource
    Copy-Item -LiteralPath $agentSource -Destination $agentSidecar -Force

    pnpm --dir $uiDirectory install --frozen-lockfile
    if ($LASTEXITCODE -ne 0) { throw "Desktop dependency installation failed" }

    Push-Location $desktopDirectory
    try {
        $tauriArguments = @("build", "--ci", "--config", $releaseConfig)
        if ($env:REMOTEX_WINDOWS_CERTIFICATE_THUMBPRINT) {
            $generatedSigningConfig = Join-Path ([System.IO.Path]::GetTempPath()) "remotex-tauri-signing-$PID.json"
            @{
                bundle = @{ windows = @{
                    certificateThumbprint = $env:REMOTEX_WINDOWS_CERTIFICATE_THUMBPRINT
                    digestAlgorithm = "sha256"
                    timestampUrl = $env:REMOTEX_WINDOWS_TIMESTAMP_URL
                    tsp = $true
                } }
            } | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $generatedSigningConfig -Encoding utf8
            $tauriArguments += @("--config", $generatedSigningConfig)
        }
        & (Join-Path $uiDirectory "node_modules\.bin\tauri.cmd") @tauriArguments
        if ($LASTEXITCODE -ne 0) { throw "Tauri installer build failed" }
        Assert-WindowsGuiSubsystem (Join-Path $repository "target\release\remotex-desktop.exe")
    }
    finally {
        Pop-Location
    }
}
finally {
    Pop-Location
    if ($generatedSigningConfig -and (Test-Path -LiteralPath $generatedSigningConfig)) {
        Remove-Item -LiteralPath $generatedSigningConfig -Force
    }
}

Write-Host "RemoteX NSIS installer created under target\release\bundle\nsis"
