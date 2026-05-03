@echo off
setlocal
cd /d %~dp0
cargo build --release
if errorlevel 1 (
  echo [ERROR] ????
  exit /b 1
)
echo [OK] ????: .\target\release\musicdownload.exe
endlocal
