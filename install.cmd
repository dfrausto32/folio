@echo off
rem Double-click to install Folio. Works from a source checkout or from a release download.
if exist "%~dp0folio.exe" (
  rem Release download: nothing to build, so run quietly and report in a message box.
  powershell -NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File "%~dp0install.ps1" -Yes -Gui %*
  exit /b
)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
echo.
pause
