[CmdletBinding()]
param([string]$UpstreamRoot)

$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrWhiteSpace($UpstreamRoot)) {
    $UpstreamRoot = Join-Path $PSScriptRoot '../../Codex-Discord-Rich-Presence'
}
$local = Join-Path $PSScriptRoot '../src/codex/model_catalog.json'
$upstream = Join-Path $UpstreamRoot 'src/model_catalog.json'
if ((Get-FileHash -LiteralPath $local).Hash -ne (Get-FileHash -LiteralPath $upstream).Hash) {
    throw 'Model catalog parity failed: Pulse differs from the canonical runtime catalog.'
}
$catalog = Get-Content -LiteralPath $local -Raw | ConvertFrom-Json
$astra = @($catalog.models | Where-Object id -eq 'gpt-6-astra')
if ($astra.Count -ne 1 -or $astra[0].display_name -ne 'GPT-6 Astra' -or $astra[0].context.max_input_tokens -ne 922000) {
    throw 'Astra catalog contract failed.'
}
Write-Output 'MODEL CATALOG PARITY OK'
