@echo off
rem Freebuff2API launcher - ASCII safe version
rem Start gateway from release or root, keep window open

title Freebuff2API Gateway

cd /d "%~dp0"

set BIN=
if exist "target\release\freebuff2api.exe" set BIN=target\release\freebuff2api.exe
if not defined BIN if exist "freebuff2api.exe" set BIN=freebuff2api.exe
if not defined BIN goto :no_bin

if not exist "config.json" goto :no_cfg

echo ==========================================
echo   Freebuff2API v0.1.0 (Rust Gateway)
echo ==========================================
echo   [OK] Using config.json
echo   [UI]  http://127.0.0.1:8787/ui
echo   [HZ]  http://127.0.0.1:8787/healthz
echo.
echo   Press Ctrl+C to stop. Window stays open.
echo.
"%BIN%" --config config.json
echo.
echo   Gateway stopped.
pause
exit /b 0

:no_bin
echo [ERROR] freebuff2api.exe not found.
echo Build first: run build.bat, or download a release.
pause
exit /b 1

:no_cfg
echo [WARN] config.json not found - starting with defaults.
echo.
"%BIN%"
pause
exit /b 0
