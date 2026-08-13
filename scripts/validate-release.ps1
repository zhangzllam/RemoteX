param([switch]$Artifacts)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$scriptDirectory = Split-Path -Parent $MyInvocation.MyCommand.Path
$repository = Resolve-Path (Join-Path $scriptDirectory "..")
$workspace = Get-Content -LiteralPath (Join-Path $repository "Cargo.toml") -Raw
$match = [regex]::Match($workspace, '(?ms)\[workspace\.package\].*?^version\s*=\s*"([^"]+)"')
if (-not $match.Success) { throw "Workspace version was not found" }
$version = $match.Groups[1].Value
$ui = Get-Content -LiteralPath (Join-Path $repository "apps\desktop\ui\package.json") -Raw | ConvertFrom-Json
$tauri = Get-Content -LiteralPath (Join-Path $repository "apps\desktop\tauri.conf.json") -Raw | ConvertFrom-Json
if ($ui.version -ne $version) { throw "UI version $($ui.version) does not match workspace $version" }
if ($tauri.version -ne $version) { throw "Tauri version $($tauri.version) does not match workspace $version" }

if ($env:GITHUB_REF_TYPE -eq "tag") {
    $expectedTag = "v$version"
    if ($env:GITHUB_REF_NAME -ne $expectedTag) {
        throw "Tag $($env:GITHUB_REF_NAME) does not match version $expectedTag"
    }
}

if ($Artifacts) {
    $bundle = Join-Path $repository "target\release\bundle\nsis"
    $installers = @(Get-ChildItem -LiteralPath $bundle -Filter "RemoteX_${version}_x64-setup.exe" -File)
    if ($installers.Count -ne 1) { throw "Expected exactly one versioned RemoteX $version installer" }
    $desktop = Join-Path $repository "target\release\remotex-desktop.exe"
    $agent = Join-Path $repository "target\release\remotex-agent.exe"
    foreach ($binary in @($desktop, $agent)) {
        if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) { throw "Missing release binary $binary" }
    }
}

Write-Host "RemoteX release metadata is consistent at version $version"

