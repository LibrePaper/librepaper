# Install the Windows local companion from a GitHub release.
$ErrorActionPreference = 'Stop'
$repo = 'LibrePaper/librepaper'
$version = if ($env:LIBREPAPER_VERSION) { $env:LIBREPAPER_VERSION } else { 'latest' }
$arch = if ([Environment]::Is64BitOperatingSystem) { 'amd64' } else { throw '32-bit Windows is not supported' }
$release = if ($version -eq 'latest') { Invoke-RestMethod "https://api.github.com/repos/$repo/releases/latest" } else { Invoke-RestMethod "https://api.github.com/repos/$repo/releases/tags/$version" }
$asset = "librepaper_windows_$arch.zip"
$entry = $release.assets | Where-Object { $_.name -eq $asset } | Select-Object -First 1
if (-not $entry) { throw "Release does not contain $asset" }
$dir = Join-Path $env:LOCALAPPDATA 'LibrePaper'
New-Item -ItemType Directory -Force $dir | Out-Null
$old = Join-Path $dir 'librepaper.exe'
$temp = Join-Path $env:TEMP ('librepaper-install-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force $temp | Out-Null
trap { Remove-Item $temp -Recurse -Force -ErrorAction SilentlyContinue; throw }
$archive = Join-Path $temp $asset
Invoke-WebRequest $entry.browser_download_url -OutFile $archive
$checksums = Join-Path $temp 'checksums.txt'
Invoke-WebRequest "https://github.com/$repo/releases/download/$($release.tag_name)/checksums.txt" -OutFile $checksums
$expected = (Get-Content $checksums | Where-Object { $_ -match "\s$([regex]::Escape($asset))$" } | Select-Object -First 1) -replace '^\s*([0-9a-fA-F]+).*','$1'
if (-not $expected) { throw "Release checksum does not contain $asset" }
$actual = (Get-FileHash $archive -Algorithm SHA256).Hash
if ($actual -ine $expected) { throw "Checksum mismatch for $asset" }
if (Test-Path $old) { & $old local stop; if ($LASTEXITCODE -ne 0) { throw 'Stop the previous companion before updating.' } }
Expand-Archive $archive -DestinationPath $dir -Force
$exe = Join-Path $dir 'librepaper.exe'
$scheme = 'HKCU:\Software\Classes\librepaper'
New-Item $scheme -Force | Out-Null
Set-Item $scheme -Value 'URL:LibrePaper connection'
New-ItemProperty $scheme -Name 'URL Protocol' -Value '' -PropertyType String -Force | Out-Null
$command = Join-Path $scheme 'shell\open\command'
New-Item $command -Force | Out-Null
Set-Item $command -Value ('"' + $exe + '" local open "%1"')
& $exe local launch
if ($LASTEXITCODE -ne 0) { throw 'The companion could not start.' }
$shortcut = Join-Path ([Environment]::GetFolderPath('Desktop')) 'LibrePaper Companion.lnk'
$shell = New-Object -ComObject WScript.Shell
$link = $shell.CreateShortcut($shortcut)
$link.TargetPath = $exe
$link.Arguments = 'local manage'
$link.WorkingDirectory = $dir
$link.Save()
Remove-Item $temp -Recurse -Force -ErrorAction SilentlyContinue
Write-Host "Installed and started LibrePaper local companion in $dir"
Start-Process $exe -ArgumentList 'local','manage'
Write-Host 'Enable Start at login in the companion settings if desired.'
