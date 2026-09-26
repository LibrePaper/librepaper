@echo off
setlocal
if exist "%~dp0install-companion.ps1" goto local_installer

set "SCRIPT=%TEMP%\librepaper-install-%RANDOM%.ps1"
powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "$ErrorActionPreference='Stop'; Invoke-WebRequest -UseBasicParsing 'https://raw.githubusercontent.com/LibrePaper/librepaper/main/deploy/install-companion.ps1' -OutFile $env:SCRIPT"
if errorlevel 1 exit /b 1
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%SCRIPT%" %*
set "RESULT=%ERRORLEVEL%"
del "%SCRIPT%" >nul 2>nul
exit /b %RESULT%

:local_installer
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install-companion.ps1" %*
exit /b %ERRORLEVEL%
