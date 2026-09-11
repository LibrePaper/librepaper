@echo off
setlocal
if exist "%~dp0install-companion.ps1" (
  powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install-companion.ps1"
) else (
  powershell.exe -NoProfile -ExecutionPolicy Bypass -Command "$release=Invoke-RestMethod 'https://api.github.com/repos/LibrePaper/librepaper/releases/latest'; $asset=@($release.assets).Where({$_.name -eq 'install-companion.ps1'})[0]; if(-not $asset){throw 'Companion installer not found'}; $env:LIBREPAPER_VERSION=$release.tag_name; ([scriptblock]::Create((Invoke-WebRequest -UseBasicParsing $asset.browser_download_url).Content)).Invoke()"
)
if errorlevel 1 pause
