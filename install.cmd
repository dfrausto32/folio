@echo off
rem Double-click to install Folio. Works from a source checkout or from a release download.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
echo.
pause
