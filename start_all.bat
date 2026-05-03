@echo off
setlocal
cd /d %~dp0

if not exist ".\target\release\musicdownload.exe" (
  echo [ERROR] ??? .\target\release\musicdownload.exe????? cargo build --release
  exit /b 1
)

for /f "tokens=5" %%a in ('netstat -ano ^| findstr ":3001" ^| findstr "LISTENING"') do taskkill /PID %%a /F >nul 2>nul
for /f "tokens=5" %%a in ('netstat -ano ^| findstr ":8080" ^| findstr "LISTENING"') do taskkill /PID %%a /F >nul 2>nul

start "meting-local" /D "%~dp0meting-local" "C:\Program Files\nodejs\node.exe" server.mjs
ping 127.0.0.1 -n 3 >nul
start "musicdownload-backend" /D "%~dp0" "%~dp0target\release\musicdownload.exe"

echo [OK] ???:
echo   - Meting:  http://127.0.0.1:3001/api
echo   - Backend: http://127.0.0.1:8080
echo   - Frontend:http://127.0.0.1:8080
endlocal
