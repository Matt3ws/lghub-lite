@echo off
rem peripheral-lite diagnostics: double-click, then send the file that opens (no personal data,
rem only device names, IDs and HID details).
cd /d "%~dp0"
set "OUT=%~dp0peripheral-lite-report.txt"
echo Collecting peripheral-lite diagnostics, this takes a few seconds...
"%~dp0peripheral-lite.exe" report > "%OUT%" 2>&1
echo.
echo Saved: %OUT%
echo Please send this file (or paste its text) to whoever asked for it.
start "" notepad "%OUT%"
