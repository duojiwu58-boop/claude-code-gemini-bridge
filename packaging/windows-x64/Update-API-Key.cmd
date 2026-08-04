@echo off
chcp 65001 >nul
title Claude Code Bridge API Key
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0update-api-key.ps1"
if errorlevel 1 (
  echo.
  echo 更新失败，请查看上面的错误信息。
  pause
  exit /b 1
)
echo.
echo API Key 已更新，服务已重新启动。
pause
