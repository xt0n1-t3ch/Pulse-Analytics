#requires -Version 5.1
<#
.SYNOPSIS
  Build, verify, and publish an immutable Pulse Windows release locally.

.DESCRIPTION
  Builds the frontend and Tauri installers, generates and validates the Windows
  SPDX SBOM, selects only installers matching the exact package version, writes
  SHA256SUMS.txt, uploads a draft, verifies the downloaded bytes, and only then
  makes the GitHub release public. Every check and asset step is a `cargo xtask`
  command, so this script holds only the Windows-specific orchestration.

.PARAMETER WindowsOnlyRecovery
  Explicitly acknowledge a Windows x64-only recovery release. This lane never
  becomes the latest release and cannot replace the six-platform release lane.

.PARAMETER Tag
  Release tag, for example v1.7.2. Defaults to "v" plus package.json.version.

.PARAMETER Draft
  Keep the verified GitHub release as a draft instead of publishing it.

.PARAMETER SkipBuild
  Reuse exact-version installers already present under target/release/bundle.
#>
param(
  [string]$Tag,
  [switch]$Draft,
  [switch]$SkipBuild,
  [switch]$WindowsOnlyRecovery
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
if (-not $WindowsOnlyRecovery) {
  throw "Use the manual Release workflow for Windows, macOS and Linux. This script requires -WindowsOnlyRecovery and never publishes as latest."
}
if ([System.Environment]::OSVersion.Platform -ne [System.PlatformID]::Win32NT -or
    [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString() -ne 'X64') {
  throw 'The local recovery lane requires a Windows x64 host'
}

$root = Split-Path -Parent $PSScriptRoot
$package = Get-Content -Raw -LiteralPath (Join-Path $root "package.json") | ConvertFrom-Json
$version = [string]$package.version
$expectedTag = "v$version"
if (-not $Tag) { $Tag = $expectedTag }
if ($Tag -ne $expectedTag) {
  throw "Release tag $Tag does not match package version $version ($expectedTag)"
}

$bundle = Join-Path $root "target/release/bundle"
$nsis = Join-Path $bundle "nsis/Pulse_${version}_x64-setup.exe"
$msi = Join-Path $bundle "msi/Pulse_${version}_x64_en-US.msi"
$sbom = Join-Path $bundle "pulse-windows-x64.spdx.json"
$output = Join-Path $root "target/release/local-release/$Tag"
$verification = Join-Path ([System.IO.Path]::GetTempPath()) ("pulse-release-verify-" + [guid]::NewGuid().ToString("N"))
$releaseAttempted = $false
$releaseFinalized = $false
$keepVerifiedDraft = $false

function Get-RequiredRepository {
  $output = @(& gh repo view --json nameWithOwner --jq .nameWithOwner 2>&1)
  if ($LASTEXITCODE -ne 0) {
    throw "Unable to resolve the GitHub repository: $($output -join ' ')"
  }
  $repository = ($output -join "`n").Trim()
  if ($repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') {
    throw "GitHub returned an invalid repository identity: $repository"
  }
  $repository
}

function Get-ReleaseHttpStatus {
  param(
    [Parameter(Mandatory)] [string]$Repository,
    [Parameter(Mandatory)] [string]$ReleaseTag
  )

  $response = @(& gh api -i "repos/$Repository/releases/tags/$ReleaseTag" 2>&1)
  $exitCode = $LASTEXITCODE
  $statusLine = $response | Where-Object { [string]$_ -match '^HTTP/\S+\s+\d{3}\b' } | Select-Object -First 1
  if ($null -eq $statusLine) {
    throw "GitHub release preflight returned no HTTP status (exit $exitCode)"
  }
  $statusMatch = [regex]::Match([string]$statusLine, '^HTTP/\S+\s+(?<status>\d{3})\b')
  $status = [int]$statusMatch.Groups['status'].Value
  if (($status -eq 200 -and $exitCode -ne 0) -or ($status -eq 404 -and $exitCode -eq 0)) {
    throw "GitHub release preflight returned inconsistent status $status / exit $exitCode"
  }
  $status
}

function Assert-RemoteAnnotatedTag {
  param(
    [Parameter(Mandatory)] [string]$ReleaseTag,
    [Parameter(Mandatory)] [string]$ExpectedCommit
  )

  $refs = @(& git ls-remote --tags origin "refs/tags/$ReleaseTag" "refs/tags/$ReleaseTag^{}" 2>&1)
  if ($LASTEXITCODE -ne 0) {
    throw "Unable to resolve remote release tag $ReleaseTag`: $($refs -join ' ')"
  }
  $escapedTag = [regex]::Escape($ReleaseTag)
  $tagObject = $refs | Where-Object { [string]$_ -match "^[0-9a-fA-F]{40}\s+refs/tags/$escapedTag$" } | Select-Object -First 1
  $peeledTag = $refs | Where-Object { [string]$_ -match "^[0-9a-fA-F]{40}\s+refs/tags/$escapedTag\^\{\}$" } | Select-Object -First 1
  if ($null -eq $tagObject -or $null -eq $peeledTag) {
    throw "Remote release tag $ReleaseTag must exist as an annotated tag"
  }
  $remoteCommit = (([string]$peeledTag -split '\s+')[0]).ToLowerInvariant()
  if ($remoteCommit -ne $ExpectedCommit) {
    throw "Remote release tag $ReleaseTag resolves to $remoteCommit instead of $ExpectedCommit"
  }
}

function Remove-FailedRelease {
  param([Parameter(Mandatory)] [string]$ReleaseTag)

  $output = @(& gh release view $ReleaseTag --json isDraft,isImmutable 2>&1)
  if ($LASTEXITCODE -ne 0) {
    Write-Warning "Unable to inspect a possibly partial release $ReleaseTag during cleanup."
    return
  }
  try { $state = ($output -join "`n") | ConvertFrom-Json }
  catch {
    Write-Warning "GitHub returned invalid release state for $ReleaseTag during cleanup."
    return
  }
  if ([bool]$state.isDraft -or -not [bool]$state.isImmutable) {
    $deleteOutput = @(& gh release delete $ReleaseTag --yes 2>&1)
    if ($LASTEXITCODE -ne 0) {
      Write-Warning "Unable to delete failed release $ReleaseTag`: $($deleteOutput -join ' ')"
    }
  } else {
    Write-Warning "Release $ReleaseTag is already immutable; failure cleanup left it intact."
  }
}

Write-Host "Preparing Pulse $Tag ..." -ForegroundColor Cyan

Push-Location $root
try {
  $repository = Get-RequiredRepository
  $releaseStatus = Get-ReleaseHttpStatus -Repository $repository -ReleaseTag $Tag
  if ($releaseStatus -eq 200) {
    throw "Release $Tag already exists; immutable releases are never updated"
  }
  if ($releaseStatus -ne 404) {
    throw "GitHub release preflight failed with HTTP $releaseStatus"
  }

  $tagOutput = @(& git rev-parse "$Tag^{commit}" 2>&1)
  if ($LASTEXITCODE -ne 0) {
    throw "Release tag $Tag does not resolve to a commit: $($tagOutput -join ' ')"
  }
  $tagCommit = (($tagOutput | Select-Object -First 1).Trim()).ToLowerInvariant()
  if ($tagCommit -notmatch '^[0-9a-f]{40}$') {
    throw "Release tag $Tag returned an invalid commit: $tagCommit"
  }
  Assert-RemoteAnnotatedTag -ReleaseTag $Tag -ExpectedCommit $tagCommit
  cargo xtask release-contract --tag $Tag --expected-commit $tagCommit
  if ($LASTEXITCODE -ne 0) { throw "Release contract validation failed" }
  cargo xtask upstream check
  if ($LASTEXITCODE -ne 0) { throw "Codex presence core contract validation failed" }

  if (-not $SkipBuild) {
    $buildStartedAt = [DateTime]::UtcNow.AddSeconds(-2)
    Write-Host "Building exact-version installers..." -ForegroundColor Cyan
    bun run build
    $buildExitCode = $LASTEXITCODE
    foreach ($installer in @($nsis, $msi)) {
      if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
        throw "Build did not produce required installer: $installer"
      }
      if ((Get-Item -LiteralPath $installer).LastWriteTimeUtc -lt $buildStartedAt) {
        throw "Build did not refresh required installer: $installer"
      }
    }
    if ($buildExitCode -ne 0) {
      Write-Warning "The optional updater-signing phase exited $buildExitCode after fresh installers were produced; continuing with manual-install assets."
    }
  }

  foreach ($installer in @($nsis, $msi)) {
    if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
      throw "Required exact-version installer is missing: $installer"
    }
  }

  cargo xtask sbom new --artifact $nsis --output $sbom --package-name pulse --package-version $version
  if ($LASTEXITCODE -ne 0) { throw "Windows SBOM generation failed" }
  cargo xtask sbom check --artifact $nsis --sbom $sbom --package-name pulse --package-version $version
  if ($LASTEXITCODE -ne 0) { throw "Windows SBOM validation failed" }

  cargo xtask local-release-assets --bundle $bundle --output $output --tag $Tag
  if ($LASTEXITCODE -ne 0) { throw "Local release asset preparation failed" }

  $assets = @(Get-ChildItem -LiteralPath $output -File | Sort-Object Name)
  if ($assets.Count -ne 4) {
    throw "Expected exactly four local release assets, found $($assets.Count)"
  }
  $notesOutput = @(cargo xtask changelog notes $version 2>&1)
  if ($LASTEXITCODE -ne 0) { throw "Release note extraction failed: $($notesOutput -join ' ')" }
  $notes = ($notesOutput -join "`n").Trim()
  if ([string]::IsNullOrWhiteSpace($notes)) { throw "Release notes are empty for $version" }
  $arguments = @(
    "release", "create", $Tag,
    "--draft", "--verify-tag", "--latest=false",
    "--title", "Pulse $Tag (Windows x64 recovery)",
    "--notes", ("Windows x64 recovery release. This release is not the cross-platform latest release and has no in-app updater manifest.`n`n" + $notes)
  )
  $arguments += @($assets | ForEach-Object { $_.FullName })
  $releaseAttempted = $true
  gh @arguments
  if ($LASTEXITCODE -ne 0) { throw "GitHub draft release creation failed" }

  New-Item -ItemType Directory -Path $verification | Out-Null
  gh release download $Tag --dir $verification
  if ($LASTEXITCODE -ne 0) { throw "Published asset download verification failed" }
  $expectedNames = @($assets | ForEach-Object Name | Sort-Object)
  $actualNames = @(Get-ChildItem -LiteralPath $verification -File | ForEach-Object Name | Sort-Object)
  if (($expectedNames -join "`n") -ne ($actualNames -join "`n")) {
    throw "Published asset inventory differs from the local verified set"
  }
  foreach ($asset in $assets) {
    $downloaded = Join-Path $verification $asset.Name
    $expectedHash = (Get-FileHash -LiteralPath $asset.FullName -Algorithm SHA256).Hash
    $actualHash = (Get-FileHash -LiteralPath $downloaded -Algorithm SHA256).Hash
    if ($actualHash -ne $expectedHash) {
      throw "Published asset hash mismatch: $($asset.Name)"
    }
  }

  if ($Draft) {
    $keepVerifiedDraft = $true
    Write-Host "Verified draft retained: $Tag" -ForegroundColor Yellow
  } else {
    gh release edit $Tag --draft=false --latest=false
    if ($LASTEXITCODE -ne 0) { throw "GitHub release finalization failed" }
    $immutableOutput = @(& gh api "repos/$repository/releases/tags/$Tag" --jq .immutable 2>&1)
    if ($LASTEXITCODE -ne 0) {
      throw "Unable to verify immutable release state: $($immutableOutput -join ' ')"
    }
    $immutable = ($immutableOutput -join "`n").Trim()
    if ($immutable -ne "true") {
      throw "GitHub release $Tag is public but not immutable"
    }
    $releaseFinalized = $true
    Write-Host "Released and verified immutable: $Tag" -ForegroundColor Green
  }
} finally {
  if (Test-Path -LiteralPath $verification) {
    $temporaryRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd([System.IO.Path]::DirectorySeparatorChar) + [System.IO.Path]::DirectorySeparatorChar
    $resolvedVerification = [System.IO.Path]::GetFullPath($verification)
    if (-not $resolvedVerification.StartsWith($temporaryRoot, [System.StringComparison]::OrdinalIgnoreCase) -or [System.IO.Path]::GetFileName($resolvedVerification) -notmatch '^pulse-release-verify-[0-9a-f]{32}$') {
      throw "Refusing cleanup outside the task-owned release verification directory"
    }
    Remove-Item -LiteralPath $resolvedVerification -Recurse -Force
  }
  if ($releaseAttempted -and -not $releaseFinalized -and -not $keepVerifiedDraft) {
    Remove-FailedRelease -ReleaseTag $Tag
  }
  Pop-Location
}
