@echo off
setlocal
call "C:\Program Files (x86)\Embarcadero\Studio\22.0\bin\rsvars.bat"
if errorlevel 1 exit /b %errorlevel%
msbuild "%~dp0ClaudeBridgeManager.dproj" /t:Build /p:Config=Release /p:Platform=Win32 /m
exit /b %errorlevel%
