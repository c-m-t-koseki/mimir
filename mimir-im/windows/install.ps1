# Build Mimir for Windows and register it as a text service.
# Run from an elevated PowerShell (TSF registration writes HKLM/HKCR):
#   .\mimir-im\windows\install.ps1            set up the build tools, install / update
#   .\mimir-im\windows\install.ps1 -Setup     only set up the build tools
#   .\mimir-im\windows\install.ps1 -Uninstall unregister and remove
param([switch]$Setup, [switch]$Uninstall)
$ErrorActionPreference = "Stop"

# --- Build tools -------------------------------------------------------------
# Rust (MSVC), the Visual Studio C++ build tools, CMake (llama.cpp) and LLVM
# (libclang, for llama.cpp's bindgen), and the WebView2 runtime the settings
# window draws in. What is missing is installed with winget, after asking once.

function Add-Path($dir) {
    if ((Test-Path $dir) -and ($env:Path -split ";" -notcontains $dir)) { $env:Path = "$dir;$env:Path" }
}

function Find-VCTools {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (-not (Test-Path $vswhere)) { return $null }
    & $vswhere -products * -latest -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
}

function Find-Libclang {
    foreach ($dir in @($env:LIBCLANG_PATH, (Join-Path $env:ProgramFiles "LLVM\bin"))) {
        if ($dir -and (Test-Path (Join-Path $dir "libclang.dll"))) { return $dir }
    }
    return $null
}

function Find-WebView2 {
    # The Evergreen runtime's client key, machine-wide or per user.
    $client = "Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
    foreach ($key in @("HKLM:\SOFTWARE\WOW6432Node\$client", "HKLM:\SOFTWARE\$client", "HKCU:\Software\$client")) {
        $version = (Get-ItemProperty $key -ErrorAction SilentlyContinue).pv
        if ($version -and $version -ne "0.0.0.0") { return $version }
    }
    return $null
}

function Initialize-BuildTools {
    # Tools installed earlier but not on this session's PATH yet.
    Add-Path (Join-Path $env:USERPROFILE ".cargo\bin")
    Add-Path (Join-Path $env:ProgramFiles "CMake\bin")

    $missing = @()
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        $missing += @{ Name = "Rust"; Id = "Rustlang.Rustup"; Args = @() }
    }
    if (-not (Find-VCTools)) {
        $missing += @{ Name = "Visual Studio Build Tools (C++)"; Id = "Microsoft.VisualStudio.2022.BuildTools";
            Args = @("--override", "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended") }
    }
    if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
        $missing += @{ Name = "CMake"; Id = "Kitware.CMake"; Args = @() }
    }
    if (-not (Find-Libclang)) {
        $missing += @{ Name = "LLVM (libclang)"; Id = "LLVM.LLVM"; Args = @() }
    }
    if (-not (Find-WebView2)) {
        $missing += @{ Name = "WebView2 Runtime"; Id = "Microsoft.EdgeWebView2Runtime"; Args = @() }
    }

    if ($missing) {
        if (-not (Get-Command winget -ErrorAction SilentlyContinue)) {
            throw "winget not found: install App Installer from the Microsoft Store, or install $(($missing | ForEach-Object { $_.Name }) -join ', ') by hand."
        }
        Write-Host "Missing build tools: $(($missing | ForEach-Object { $_.Name }) -join ', ')"
        if ((Read-Host "Install them with winget? [y/N]") -notmatch "^[yY]") { throw "Build tools missing." }
        foreach ($tool in $missing) {
            Write-Host "Installing $($tool.Name) ..."
            winget install --exact --id $tool.Id --source winget --accept-package-agreements --accept-source-agreements @($tool.Args)
            if ($LASTEXITCODE -ne 0) { throw "winget failed to install $($tool.Name) (exit $LASTEXITCODE)." }
        }
        Add-Path (Join-Path $env:USERPROFILE ".cargo\bin")
        Add-Path (Join-Path $env:ProgramFiles "CMake\bin")
    }

    # rustup's default may be the GNU toolchain; the DLL and llama.cpp want MSVC.
    rustup default stable-msvc | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "rustup failed to set up the stable MSVC toolchain." }
    $env:LIBCLANG_PATH = Find-Libclang
    foreach ($check in @(
        @{ Name = "cargo"; Ok = [bool](Get-Command cargo -ErrorAction SilentlyContinue) },
        @{ Name = "Visual Studio C++ build tools"; Ok = [bool](Find-VCTools) },
        @{ Name = "cmake"; Ok = [bool](Get-Command cmake -ErrorAction SilentlyContinue) },
        @{ Name = "libclang"; Ok = [bool]$env:LIBCLANG_PATH })) {
        if (-not $check.Ok) { throw "$($check.Name) still not found; open a new PowerShell and run this again." }
    }
    Write-Host "Build tools ready: $(cargo --version), $(cmake --version | Select-Object -First 1), libclang in $env:LIBCLANG_PATH"
}

if (-not $Uninstall) {
    Initialize-BuildTools
    if ($Setup) { return }
}

# --- Install -------------------------------------------------------------------

$root = Resolve-Path "$PSScriptRoot\..\.."
$dest = Join-Path $env:ProgramFiles "Mimir"
$dll = Join-Path $dest "mimir_tsf.dll"
$shortcut = Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs\Mimir Settings.lnk"

if (Test-Path $dll) {
    Start-Process regsvr32 -ArgumentList "/s", "/u", "`"$dll`"" -Wait
    # A DLL still loaded by running applications can't be overwritten.
    Get-Process mimir-imserver, mimir-settings -ErrorAction SilentlyContinue | Stop-Process
}
if ($Uninstall) {
    Remove-Item -Force $shortcut -ErrorAction SilentlyContinue
    Remove-Item -Recurse -Force $dest -ErrorAction SilentlyContinue
    Write-Host "Uninstalled. Sign out and back in to unload it from running applications."
    return
}

# A failing native command doesn't stop the script ($ErrorActionPreference
# covers cmdlets only): stop at the build that failed, not at the copy after it.
function Invoke-Cargo {
    cargo @args
    if ($LASTEXITCODE -ne 0) { throw "cargo $($args -join ' ') failed (exit $LASTEXITCODE); its error is above." }
}

Invoke-Cargo build --release --manifest-path "$root\Cargo.toml" -p mimir-im --bin mimir-imserver
Invoke-Cargo build --release --manifest-path "$root\Cargo.toml" -p mimir-tsf -p mimir-settings

New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item "$root\target\release\mimir_tsf.dll" $dest -Force
Copy-Item "$root\target\release\mimir-imserver.exe" $dest -Force
Copy-Item "$root\target\release\mimir-settings.exe" $dest -Force

# The settings window opens from the input indicator's menu (a right click on
# the mode icon); the Start menu entry is for pinning it too.
$link = (New-Object -ComObject WScript.Shell).CreateShortcut($shortcut)
$link.TargetPath = Join-Path $dest "mimir-settings.exe"
$link.Save()

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

$register = Start-Process regsvr32 -ArgumentList "/s", "`"$dll`"" -Wait -PassThru
if ($register.ExitCode -ne 0) { throw "regsvr32 failed to register $dll (exit $($register.ExitCode))." }
Write-Host "Installed to $dest. Add Mimir in Settings > Time & language > Language & region > Japanese > Language options > Keyboards."
