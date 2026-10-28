<#
.SYNOPSIS
  Folio installer for Windows (per-user, no administrator rights needed).

.DESCRIPTION
  From a source checkout: installs missing build tools (with your permission), builds Folio, installs it.
  From a release download (a folder that already contains folio.exe): just installs, nothing to build.

  Creates a Start Menu shortcut, a Desktop shortcut, an "Open with" entry for PDF files and an
  entry in Settings > Apps so Folio can be uninstalled normally.

.PARAMETER RemoveSource
  After installing, delete the source folder this script was run from.

.PARAMETER Yes
  Do not ask questions; accept the defaults (installs missing build tools).

.PARAMETER NoDesktopShortcut
  Skip the Desktop shortcut.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File .\install.ps1 -RemoveSource
#>
[CmdletBinding()]
param(
  [switch]$RemoveSource,
  [switch]$Yes,
  [switch]$NoDesktopShortcut,
  [switch]$Gui,   # used by install.cmd for release downloads: no console window, results shown in a message box
  [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\Folio')
)

$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'   # makes Invoke-WebRequest much faster
$PdfiumTag = 'chromium/7543'               # pinned: must match the pdfium-render API level in Cargo.toml
$Version = '0.1.0'
$SelfDir = Split-Path -Parent $MyInvocation.MyCommand.Path

function Show-Box($text, $icon) {
  Add-Type -AssemblyName System.Windows.Forms
  [void][System.Windows.Forms.MessageBox]::Show($text, 'Folio', 'OK', $icon)
}
if ($Gui) {
  trap { Show-Box "Folio could not be installed.`n`n$($_.Exception.Message)" 'Error'; exit 1 }
}

function Say($m)  { Write-Host "==> $m" -ForegroundColor Magenta }
function Warn($m) { Write-Host "!!  $m" -ForegroundColor Yellow }
function Ask($q) {
  if ($Yes) { return $true }
  $r = Read-Host "$q [Y/n]"
  return ($r -eq '' -or $r -match '^[Yy]')
}
function Refresh-Path {
  $machine = [Environment]::GetEnvironmentVariable('Path', 'Machine')
  $user = [Environment]::GetEnvironmentVariable('Path', 'User')
  $env:Path = "$machine;$user;$env:USERPROFILE\.cargo\bin"
}

if ($env:OS -ne 'Windows_NT') { throw 'This installer is for Windows. On Linux use install.sh.' }

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }

# ---- 1. Where do the program files come from? --------------------------------
if ((Test-Path (Join-Path $SelfDir 'folio.exe')) -and (Test-Path (Join-Path $SelfDir 'pdfium.dll'))) {
  $Mode = 'package'
  $ExeSrc = Join-Path $SelfDir 'folio.exe'
  $PdfiumSrc = Join-Path $SelfDir 'pdfium.dll'
  $IconSrc = Join-Path $SelfDir 'folio.ico'
  Say 'Installing from a prebuilt package.'
} else {
  $Mode = 'source'
  if (-not ((Test-Path (Join-Path $SelfDir 'Cargo.toml')) -and (Test-Path (Join-Path $SelfDir 'src-tauri')))) {
    throw 'Run this script from the Folio source folder (or from an extracted release download).'
  }
  Set-Location $SelfDir
  $ExeSrc = Join-Path $SelfDir 'target\release\folio.exe'
  $PdfiumSrc = Join-Path $SelfDir 'vendor\pdfium\bin\pdfium.dll'
  $IconSrc = Join-Path $SelfDir 'src-tauri\icons\icon.ico'
}

# ---- 2. Build prerequisites (source installs only) ---------------------------
if ($Mode -eq 'source') {
  Refresh-Path

  # Visual Studio C++ Build Tools (the Rust linker on Windows)
  $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  $haveMsvc = $false
  if (Test-Path $vswhere) {
    $found = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
    if ($found) { $haveMsvc = $true }
  }
  if (-not $haveMsvc) {
    Warn 'The Microsoft C++ Build Tools are required to build Folio (a large one-time download, several GB).'
    Warn 'Tip: skip all of this by using the prebuilt Windows zip from the project''s GitHub Releases page.'
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { throw 'winget is not available. Install "Build Tools for Visual Studio" (Desktop development with C++) manually, then re-run.' }
    if (-not (Ask 'Install Microsoft C++ Build Tools with winget now?')) { throw 'Cannot build without the C++ Build Tools.' }
    winget install --id Microsoft.VisualStudio.2022.BuildTools -e --accept-package-agreements --accept-source-agreements --override '--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended'
  }

  # WebView2 runtime (ships with Windows 11 and current Windows 10)
  $wv2 = @(
    'HKLM:\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
    'HKLM:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}',
    'HKCU:\SOFTWARE\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}'
  ) | Where-Object { Test-Path $_ }
  if (-not $wv2) {
    Warn 'Microsoft Edge WebView2 runtime was not found; Folio needs it to display its window.'
    if ((Get-Command winget -ErrorAction SilentlyContinue) -and (Ask 'Install WebView2 with winget now?')) {
      winget install --id Microsoft.EdgeWebView2Runtime -e --accept-package-agreements --accept-source-agreements
    }
  }

  # Rust
  if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Warn 'Rust is not installed.'
    if (-not (Get-Command winget -ErrorAction SilentlyContinue)) { throw 'Install Rust from https://rustup.rs and re-run.' }
    if (-not (Ask 'Install Rust with winget (rustup)?')) { throw 'Rust is required to build from source.' }
    winget install --id Rustlang.Rustup -e --accept-package-agreements --accept-source-agreements
    Refresh-Path
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { throw 'Rust was installed but is not on PATH yet. Open a new terminal and re-run this script.' }
  }

  # ---- 3. PDFium (the rendering engine) --------------------------------------
  if (-not (Test-Path $PdfiumSrc)) {
    Say "Downloading PDFium ($PdfiumTag, win-$arch)..."
    New-Item -ItemType Directory -Force -Path 'vendor\pdfium' | Out-Null
    $url = "https://github.com/bblanchon/pdfium-binaries/releases/download/$PdfiumTag/pdfium-win-$arch.tgz"
    Invoke-WebRequest -Uri $url -OutFile 'vendor\pdfium.tgz' -UseBasicParsing
    tar -xzf 'vendor\pdfium.tgz' -C 'vendor\pdfium'
    if ($LASTEXITCODE -ne 0) { throw 'Could not unpack the PDFium download (tar failed).' }
    Remove-Item 'vendor\pdfium.tgz' -Force
  }
  if (-not (Test-Path $PdfiumSrc)) { throw 'PDFium library missing after download.' }

  # ---- 4. Build ---------------------------------------------------------------
  Say 'Building Folio (first build takes several minutes)...'
  cargo build --release -p folio
  if ($LASTEXITCODE -ne 0) { throw 'Build failed. See the messages above.' }
}

if (-not (Test-Path $ExeSrc)) { throw "Build output not found: $ExeSrc" }

# ---- 5. Install ----------------------------------------------------------------
Say "Installing to $InstallDir"
Get-Process -Name folio -ErrorAction SilentlyContinue | Where-Object { $_.Path -like "$InstallDir*" } | Stop-Process -Force
New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
Copy-Item $ExeSrc (Join-Path $InstallDir 'folio.exe') -Force
Copy-Item $PdfiumSrc (Join-Path $InstallDir 'pdfium.dll') -Force
Copy-Item $IconSrc (Join-Path $InstallDir 'folio.ico') -Force
foreach ($f in 'LICENSE', 'THIRD-PARTY-NOTICES.md', 'uninstall.ps1') {
  $p = Join-Path $SelfDir $f
  if (Test-Path $p) { Copy-Item $p (Join-Path $InstallDir $f) -Force }
}
# Files that came from a downloaded zip carry a "downloaded from the internet" mark, which is what makes
# Windows show "Windows protected your PC" every time Folio starts. Clear it on the installed copies.
Get-ChildItem $InstallDir -Recurse -File | Unblock-File -ErrorAction SilentlyContinue
$Exe = Join-Path $InstallDir 'folio.exe'
$Ico = Join-Path $InstallDir 'folio.ico'

# ---- 6. Shortcuts ----------------------------------------------------------------
function New-Shortcut($path) {
  $shell = New-Object -ComObject WScript.Shell
  $lnk = $shell.CreateShortcut($path)
  $lnk.TargetPath = $Exe
  $lnk.WorkingDirectory = $InstallDir
  $lnk.IconLocation = "$Ico,0"
  $lnk.Description = 'Folio PDF viewer and editor'
  $lnk.Save()
}
$StartMenu = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs'
New-Shortcut (Join-Path $StartMenu 'Folio.lnk')
if (-not $NoDesktopShortcut) {
  New-Shortcut (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Folio.lnk')
}

# ---- 7. "Open with" for PDFs + Settings > Apps entry (current user only) ---------
$cls = 'HKCU:\Software\Classes'
New-Item -Force -Path "$cls\Folio.PDF" -Value 'PDF document (Folio)' | Out-Null
New-Item -Force -Path "$cls\Folio.PDF\DefaultIcon" -Value "`"$Ico`",0" | Out-Null
New-Item -Force -Path "$cls\Folio.PDF\shell\open\command" -Value "`"$Exe`" `"%1`"" | Out-Null
New-Item -Force -Path "$cls\Applications\folio.exe" | Out-Null
New-ItemProperty -Force -Path "$cls\Applications\folio.exe" -Name 'FriendlyAppName' -Value 'Folio' -PropertyType String | Out-Null
New-Item -Force -Path "$cls\Applications\folio.exe\shell\open\command" -Value "`"$Exe`" `"%1`"" | Out-Null
New-Item -Force -Path "$cls\.pdf\OpenWithProgids" | Out-Null
New-ItemProperty -Force -Path "$cls\.pdf\OpenWithProgids" -Name 'Folio.PDF' -Value '' -PropertyType String | Out-Null

$un = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Folio'
New-Item -Force -Path $un | Out-Null
$sizeKb = [int]((Get-ChildItem $InstallDir -Recurse | Measure-Object Length -Sum).Sum / 1KB)
$uninstallCmd = "powershell.exe -NoProfile -ExecutionPolicy Bypass -File `"$(Join-Path $InstallDir 'uninstall.ps1')`""
$props = @{
  DisplayName = 'Folio'; DisplayVersion = $Version; Publisher = 'Folio'; InstallLocation = $InstallDir
  DisplayIcon = $Ico; UninstallString = $uninstallCmd; EstimatedSize = $sizeKb
}
foreach ($k in $props.Keys) {
  $type = if ($props[$k] -is [int]) { 'DWord' } else { 'String' }
  New-ItemProperty -Force -Path $un -Name $k -Value $props[$k] -PropertyType $type | Out-Null
}
New-ItemProperty -Force -Path $un -Name 'NoModify' -Value 1 -PropertyType DWord | Out-Null
New-ItemProperty -Force -Path $un -Name 'NoRepair' -Value 1 -PropertyType DWord | Out-Null

# Tell Explorer file associations changed so icons refresh.
try {
  Add-Type -Namespace Folio -Name Shell -MemberDefinition '[DllImport("shell32.dll")] public static extern void SHChangeNotify(int e, uint f, IntPtr a, IntPtr b);'
  [Folio.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)
} catch { }

Say 'Folio is installed. Find it in the Start Menu or on your Desktop.'
if ($Gui) { Show-Box "Folio is installed.`n`nFind it in the Start Menu or on your Desktop." 'Information' }
Write-Host '    To make Folio the default PDF app: right-click a PDF > Open with > Choose another app > Folio > Always.'

# ---- 8. Optional clean-up of the source folder -----------------------------------
if ($Mode -eq 'source') {
  if (-not $RemoveSource -and -not $Yes) { $RemoveSource = Ask "Delete this source folder ($SelfDir) now? Folio keeps working without it." }
  if ($RemoveSource) {
    Set-Location $env:USERPROFILE
    # Deleting a folder that holds the running script: hand the job to a helper that waits for us to exit.
    $target = $SelfDir
    Start-Process -WindowStyle Hidden -FilePath 'cmd.exe' -ArgumentList "/c ping 127.0.0.1 -n 4 >nul & rmdir /s /q `"$target`""
    Say 'The source folder will be removed in a few seconds. (Rust stays installed; remove it from Settings > Apps if you do not need it.)'
  }
}
