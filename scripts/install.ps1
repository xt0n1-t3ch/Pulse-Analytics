# Pulse — Windows installer
# Usage: irm https://raw.githubusercontent.com/xt0n1-t3ch/Pulse/main/scripts/install.ps1 | iex
$ErrorActionPreference = 'Stop'

$Repo = 'xt0n1-t3ch/Pulse-Analytics'
$Api  = "https://api.github.com/repos/$Repo/releases/latest"

Write-Host "→ Fetching latest release..." -ForegroundColor Cyan
$release = Invoke-RestMethod -Uri $Api -Headers @{ 'User-Agent' = 'pulse-installer' }
$architecture = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
$assetArchitecture = switch ($architecture) {
  'X64' { 'x64' }
  'Arm64' { 'arm64' }
  default { throw "Unsupported Windows architecture: $architecture" }
}

$asset = $release.assets |
  Where-Object { $_.name -match "_$assetArchitecture-setup\.exe$" } |
  Select-Object -First 1

if (-not $asset) {
  $asset = $release.assets |
    Where-Object { $_.name -match "_${assetArchitecture}_en-US\.msi$" } |
    Select-Object -First 1
}

if (-not $asset) { throw "No Windows installer found in latest release." }

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) $asset.name
Write-Host "→ Downloading $($asset.name)..." -ForegroundColor Cyan
Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $tmp -UseBasicParsing

Write-Host "→ Launching installer..." -ForegroundColor Cyan
if ($tmp.EndsWith('.msi')) {
  Start-Process msiexec.exe -ArgumentList "/i `"$tmp`"" -Wait
} else {
  Start-Process $tmp -Wait
}

Write-Host "✓ Pulse installed. Launch it from the Start menu." -ForegroundColor Green
