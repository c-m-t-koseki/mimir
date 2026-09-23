# Build Mimir for Windows and register it as a text service.
# Run from an elevated PowerShell (TSF registration writes HKLM/HKCR):
#   .\mimir-im\windows\install.ps1            install / update
#   .\mimir-im\windows\install.ps1 -Uninstall unregister and remove
param([switch]$Uninstall)
$ErrorActionPreference = "Stop"

$root = Resolve-Path "$PSScriptRoot\..\.."
$dest = Join-Path $env:ProgramFiles "Mimir"
$dll = Join-Path $dest "mimir_tsf.dll"

if (Test-Path $dll) {
    Start-Process regsvr32 -ArgumentList "/s", "/u", "`"$dll`"" -Wait
    # A DLL still loaded by running applications can't be overwritten.
    Get-Process mimir-imserver -ErrorAction SilentlyContinue | Stop-Process
}
if ($Uninstall) {
    Remove-Item -Recurse -Force $dest -ErrorAction SilentlyContinue
    Write-Host "Uninstalled. Sign out and back in to unload it from running applications."
    return
}

cargo build --release --manifest-path "$root\Cargo.toml" -p mimir-im --bin mimir-imserver
cargo build --release --manifest-path "$root\Cargo.toml" -p mimir-tsf

New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item "$root\target\release\mimir_tsf.dll" $dest -Force
Copy-Item "$root\target\release\mimir-imserver.exe" $dest -Force

# System dictionary, unless one is installed (the IME works model-only without it).
$data = Join-Path $env:APPDATA "mimir\mimir-im\data"
if (-not (Test-Path "$data\dict.bin")) {
    New-Item -ItemType Directory -Force $data | Out-Null
    $tmp = Join-Path $env:TEMP "mimir-dict"
    New-Item -ItemType Directory -Force $tmp | Out-Null
    try {
        Invoke-WebRequest "https://github.com/togatoga/karukan/releases/latest/download/dict.tgz" -OutFile "$tmp\dict.tgz"
        tar -xzf "$tmp\dict.tgz" -C $tmp
        Copy-Item "$tmp\dict.bin" $data
    } catch {
        Write-Warning "System dictionary download failed; install it by hand (docs/dictionary.md)."
    }
    Remove-Item -Recurse -Force $tmp
}

# Models into the HuggingFace cache, so the first key doesn't wait on a download.
& "$dest\mimir-imserver.exe" --prefetch-models

Start-Process regsvr32 -ArgumentList "/s", "`"$dll`"" -Wait
Write-Host "Installed to $dest. Add Mimir in Settings > Time & language > Language & region > Japanese > Language options > Keyboards."
