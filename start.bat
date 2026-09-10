@echo off
rem Freebuff2API launcher - ASCII safe version
rem Start gateway from release or root, auto-open dashboard, keep window open

title Freebuff2API Gateway

cd /d "%~dp0"

set PORT=47821
set BIN=
if exist "target\release\freebuff2api.exe" set BIN=target\release\freebuff2api.exe
if not defined BIN if exist "freebuff2api.exe" set BIN=freebuff2api.exe
if not defined BIN goto :no_bin

rem ---- Already running? Just open the dashboard ----
netstat -ano | findstr ":%PORT% " | findstr "LISTENING" >nul 2>&1
if not errorlevel 1 (
  echo ==========================================
  echo   Freebuff2API Gateway is already running
  echo ==========================================
  echo   Opening dashboard: http://127.0.0.1:%PORT%/ui
  echo.
  start "" http://127.0.0.1:%PORT%/ui
  timeout /t 2 /nobreak >nul
  exit /b 0
)

if not exist "config.json" goto :no_cfg

echo ==========================================
echo   Freebuff2API (Rust Gateway)
echo ==========================================
echo   [OK] Using config.json
echo   [UI]  http://127.0.0.1:%PORT%/ui
echo   [HZ]  http://127.0.0.1:%PORT%/healthz
echo.
echo   Dashboard will open automatically in 3 seconds.
echo   Press Ctrl+C to stop. Window stays open.
echo.

rem ---- Auto-open dashboard after gateway has time to start ----
start "" /min powershell -NoProfile -WindowStyle Hidden -Command "Start-Sleep -Seconds 3; Start-Process 'http://127.0.0.1:%PORT%/ui'" >nul 2>&1

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
start "" /min powershell -NoProfile -WindowStyle Hidden -Command "Start-Sleep -Seconds 3; Start-Process 'http://127.0.0.1:%PORT%/ui'" >nul 2>&1
"%BIN%"
pause
exit /b 0
