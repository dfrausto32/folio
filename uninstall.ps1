<#
.SYNOPSIS
  Removes Folio: program files, shortcuts, "Open with" entry and the Settings > Apps entry.
  Your PDF files are never touched.
#>
[CmdletBinding()]
param([switch]$Quiet)
$ErrorActionPreference = 'SilentlyContinue'

$InstallDir = Join-Path $env:LOCALAPPDATA 'Programs\Folio'

if (-not $Quiet) {
  $r = Read-Host 'Remove Folio from this computer? [Y/n]'
  if ($r -ne '' -and $r -notmatch '^[Yy]') { return }
}

Get-Process -Name folio | Where-Object { $_.Path -like "$InstallDir*" } | Stop-Process -Force

Remove-Item (Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Folio.lnk') -Force
Remove-Item (Join-Path ([Environment]::GetFolderPath('Desktop')) 'Folio.lnk') -Force

$cls = 'HKCU:\Software\Classes'
Remove-Item "$cls\Folio.PDF" -Recurse -Force
Remove-Item "$cls\Applications\folio.exe" -Recurse -Force
Remove-ItemProperty -Path "$cls\.pdf\OpenWithProgids" -Name 'Folio.PDF' -Force
Remove-Item 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Folio' -Recurse -Force

# App data the web view created (saved theme and caches). The folder name is the app identifier.
Remove-Item (Join-Path $env:LOCALAPPDATA 'io.github.folio.pdf') -Recurse -Force
Remove-Item (Join-Path $env:APPDATA 'io.github.folio.pdf') -Recurse -Force

# This script may be running from inside the install folder, so let a helper delete it after we exit.
Start-Process -WindowStyle Hidden -FilePath 'cmd.exe' -ArgumentList "/c ping 127.0.0.1 -n 3 >nul & rmdir /s /q `"$InstallDir`""
if (-not $Quiet) { Write-Host 'Folio has been removed.' }
