# Compatibility entry point for saved Windows installer links. Cargo Dist's
# generated PowerShell installer owns release selection and installation.
$ErrorActionPreference = 'Stop'
$repo = 'LibrePaper/librepaper'
$installer = 'librepaper-installer.ps1'
$version = $env:LIBREPAPER_VERSION

if ($env:LIBREPAPER_BIN_DIR) {
  $env:LIBREPAPER_INSTALL_DIR = $env:LIBREPAPER_BIN_DIR
}

if ([string]::IsNullOrWhiteSpace($version) -or $version -eq 'latest') {
  $url = "https://github.com/$repo/releases/latest/download/$installer"
} else {
  if ($version -notmatch '^[A-Za-z0-9._-]+$') { throw "Invalid release version: $version" }
  $url = "https://github.com/$repo/releases/download/$version/$installer"
}

$content = (Invoke-WebRequest -UseBasicParsing $url).Content
$installerBlock = [scriptblock]::Create($content)
& $installerBlock @args
