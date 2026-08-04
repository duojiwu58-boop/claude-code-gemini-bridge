@echo off
chcp 65001 >nul
title Claude Code Bridge Uninstaller
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0uninstall.ps1"
if errorlevel 1 (
  echo.
  echo 卸载失败，请查看上面的错误信息。
  pause
  exit /b 1
)
echo.
echo 服务已卸载。API Key、日志和 Claude 配置均已保留。
pause
